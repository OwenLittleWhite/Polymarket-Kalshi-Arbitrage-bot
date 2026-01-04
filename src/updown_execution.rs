use anyhow::{Result, anyhow};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::dump_detector::DumpSide;
use crate::polymarket::BookSnapshot;
use crate::polymarket_clob::SharedAsyncClient;

/// Up/Down 策略执行引擎（通用，支持 BTC/ETH/SOL 等）
///
/// **设计原则**：
/// - 最小化改动原 execution.rs
/// - 专注于单平台 Polymarket 交易
/// - 支持模拟模式（dry_run = 真实数据 + 假下单）
/// - 检查流动性后再下单
pub struct UpDownExecutionEngine {
    /// Polymarket 异步客户端（复用现有代码）
    poly_async: Arc<SharedAsyncClient>,

    /// 模拟模式标志（true = 不实际下单，仅记录）
    pub dry_run: bool,

    /// 最小流动性要求（份额数）
    min_liquidity_shares: f64,

    /// 模拟成交记录（用于 dry_run 模式）
    simulated_fills: Arc<RwLock<Vec<SimulatedFill>>>,
}

/// 模拟成交记录（dry_run 模式使用）
#[derive(Debug, Clone)]
pub struct SimulatedFill {
    pub side: String,           // "Up" | "Down"
    pub action: String,         // "Buy" | "Sell"
    pub token_id: String,
    pub price: f64,             // 0.0-1.0
    pub size: f64,              // 份额数
    pub timestamp_ns: u64,
}

/// 执行结果
#[derive(Debug)]
pub struct ExecutionResult {
    pub success: bool,
    pub filled_size: f64,
    pub avg_price: f64,
    pub error_msg: Option<String>,
}

impl UpDownExecutionEngine {
    /// 创建新的 Up/Down 执行引擎
    ///
    /// # 参数
    /// - `poly_async`: Polymarket CLOB 客户端
    /// - `dry_run`: 是否启用模拟模式
    /// - `min_liquidity_shares`: 最小流动性要求（例如 20.0）
    pub fn new(
        poly_async: Arc<SharedAsyncClient>,
        dry_run: bool,
        min_liquidity_shares: f64,
    ) -> Self {
        Self {
            poly_async,
            dry_run,
            min_liquidity_shares,
            simulated_fills: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// **Phase 1**: 执行第一阶段买入
    ///
    /// 抛售检测后，买入单侧（Up 或 Down）
    ///
    /// # 参数
    /// - `side`: 买入侧（DumpSide::Up 或 DumpSide::Down）
    /// - `token_id`: 目标 token ID
    /// - `expected_price`: 期望价格（BPS，0-10000）
    /// - `shares`: 购买份额
    /// - `orderbook`: 当前订单簿快照
    /// - `timestamp_ns`: 当前时间戳
    ///
    /// # 流动性检查
    /// - 检查 orderbook.asks 的流动性
    /// - 如果流动性不足，返回错误
    pub async fn execute_leg1_buy(
        &self,
        side: DumpSide,
        token_id: &str,
        expected_price: u16,
        shares: f64,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<ExecutionResult> {
        let side_str = match side {
            DumpSide::Up => "Up",
            DumpSide::Down => "Down",
        };

        // 1. 流动性检查
        let available_liquidity = self.calculate_available_liquidity(orderbook);

        if available_liquidity < self.min_liquidity_shares {
            warn!(
                "❌ 流动性不足: 需要 {:.2}, 可用 {:.2}",
                shares, available_liquidity
            );
            return Ok(ExecutionResult {
                success: false,
                filled_size: 0.0,
                avg_price: 0.0,
                error_msg: Some(format!(
                    "流动性不足: 需要 {:.2}, 可用 {:.2}",
                    shares, available_liquidity
                )),
            });
        }

        // 2. 价格转换（BPS → dollars）
        let price = expected_price as f64 / 10000.0;

        // 3. 模拟模式 vs 真实下单
        if self.dry_run {
            info!(
                "🧪 [模拟] 买入 {} 侧: {:.2} 份 @ ${:.4} (token: {})",
                side_str, shares, price, token_id
            );

            // 记录模拟成交
            self.record_simulated_fill(
                side_str.to_string(),
                "Buy".to_string(),
                token_id.to_string(),
                price,
                shares,
                timestamp_ns,
            )
            .await;

            Ok(ExecutionResult {
                success: true,
                filled_size: shares,
                avg_price: price,
                error_msg: None,
            })
        } else {
            // 真实下单
            info!(
                "💰 买入 {} 侧: {:.2} 份 @ ${:.4} (token: {})",
                side_str, shares, price, token_id
            );

            match self.poly_async.buy_fak(token_id, price, shares).await {
                Ok(fill_result) => {
                    info!("✅ 买入成功: {:?}", fill_result);
                    Ok(ExecutionResult {
                        success: true,
                        filled_size: shares, // 简化：假设全部成交
                        avg_price: price,
                        error_msg: None,
                    })
                }
                Err(e) => {
                    warn!("❌ 买入失败: {}", e);
                    Ok(ExecutionResult {
                        success: false,
                        filled_size: 0.0,
                        avg_price: 0.0,
                        error_msg: Some(e.to_string()),
                    })
                }
            }
        }
    }

    /// **Phase 2**: 执行对冲（或平仓）
    ///
    /// 有三种场景：
    /// 1. 正常对冲：买入对侧
    /// 2. 早期退出：卖出第一阶段持仓
    /// 3. 止损平仓：卖出第一阶段持仓
    ///
    /// # 参数
    /// - `action`: "hedge" | "early_exit" | "stop_loss"
    /// - `leg1_side`: 第一阶段买入的侧
    /// - `leg1_token_id`: 第一阶段 token ID
    /// - `opposite_token_id`: 对侧 token ID（对冲时使用）
    /// - `price`: 交易价格（BPS）
    /// - `shares`: 份额数
    /// - `orderbook`: 当前订单簿快照
    pub async fn execute_leg2_or_exit(
        &self,
        action: &str, // "hedge" | "early_exit" | "stop_loss"
        leg1_side: DumpSide,
        leg1_token_id: &str,
        opposite_token_id: Option<&str>,
        price: u16,
        shares: f64,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<ExecutionResult> {
        match action {
            "hedge" => {
                // 买入对侧
                let opp_token = opposite_token_id.ok_or_else(|| {
                    anyhow!("对冲操作需要 opposite_token_id")
                })?;

                self.execute_buy_opposite(
                    leg1_side,
                    opp_token,
                    price,
                    shares,
                    orderbook,
                    timestamp_ns,
                )
                .await
            }
            "early_exit" | "stop_loss" => {
                // 卖出第一阶段持仓
                self.execute_sell_position(
                    leg1_side,
                    leg1_token_id,
                    price,
                    shares,
                    orderbook,
                    timestamp_ns,
                )
                .await
            }
            _ => Err(anyhow!("未知操作类型: {}", action)),
        }
    }

    /// 买入对侧（对冲操作）
    async fn execute_buy_opposite(
        &self,
        leg1_side: DumpSide,
        opposite_token_id: &str,
        price: u16,
        shares: f64,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<ExecutionResult> {
        let opposite_side_str = match leg1_side {
            DumpSide::Up => "Down",
            DumpSide::Down => "Up",
        };

        // 流动性检查
        let available_liquidity = self.calculate_available_liquidity(orderbook);

        if available_liquidity < self.min_liquidity_shares {
            return Ok(ExecutionResult {
                success: false,
                filled_size: 0.0,
                avg_price: 0.0,
                error_msg: Some(format!("流动性不足: {:.2}", available_liquidity)),
            });
        }

        let price_dollars = price as f64 / 10000.0;

        if self.dry_run {
            info!(
                "🧪 [模拟] 对冲买入 {} 侧: {:.2} 份 @ ${:.4}",
                opposite_side_str, shares, price_dollars
            );

            self.record_simulated_fill(
                opposite_side_str.to_string(),
                "Buy".to_string(),
                opposite_token_id.to_string(),
                price_dollars,
                shares,
                timestamp_ns,
            )
            .await;

            Ok(ExecutionResult {
                success: true,
                filled_size: shares,
                avg_price: price_dollars,
                error_msg: None,
            })
        } else {
            info!(
                "💰 对冲买入 {} 侧: {:.2} 份 @ ${:.4}",
                opposite_side_str, shares, price_dollars
            );

            match self
                .poly_async
                .buy_fak(opposite_token_id, price_dollars, shares)
                .await
            {
                Ok(_) => Ok(ExecutionResult {
                    success: true,
                    filled_size: shares,
                    avg_price: price_dollars,
                    error_msg: None,
                }),
                Err(e) => Ok(ExecutionResult {
                    success: false,
                    filled_size: 0.0,
                    avg_price: 0.0,
                    error_msg: Some(e.to_string()),
                }),
            }
        }
    }

    /// 卖出持仓（早期退出或止损）
    async fn execute_sell_position(
        &self,
        leg1_side: DumpSide,
        leg1_token_id: &str,
        price: u16,
        shares: f64,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<ExecutionResult> {
        let side_str = match leg1_side {
            DumpSide::Up => "Up",
            DumpSide::Down => "Down",
        };

        // 流动性检查
        let available_liquidity = self.calculate_available_liquidity(orderbook);

        if available_liquidity < self.min_liquidity_shares {
            return Ok(ExecutionResult {
                success: false,
                filled_size: 0.0,
                avg_price: 0.0,
                error_msg: Some(format!("流动性不足: {:.2}", available_liquidity)),
            });
        }

        let price_dollars = price as f64 / 10000.0;

        if self.dry_run {
            info!(
                "🧪 [模拟] 卖出 {} 侧: {:.2} 份 @ ${:.4}",
                side_str, shares, price_dollars
            );

            self.record_simulated_fill(
                side_str.to_string(),
                "Sell".to_string(),
                leg1_token_id.to_string(),
                price_dollars,
                shares,
                timestamp_ns,
            )
            .await;

            Ok(ExecutionResult {
                success: true,
                filled_size: shares,
                avg_price: price_dollars,
                error_msg: None,
            })
        } else {
            info!(
                "💰 卖出 {} 侧: {:.2} 份 @ ${:.4}",
                side_str, shares, price_dollars
            );

            match self
                .poly_async
                .sell_fak(leg1_token_id, price_dollars, shares)
                .await
            {
                Ok(_) => Ok(ExecutionResult {
                    success: true,
                    filled_size: shares,
                    avg_price: price_dollars,
                    error_msg: None,
                }),
                Err(e) => Ok(ExecutionResult {
                    success: false,
                    filled_size: 0.0,
                    avg_price: 0.0,
                    error_msg: Some(e.to_string()),
                }),
            }
        }
    }

    /// 计算可用流动性（订单簿 asks 侧）
    ///
    /// 简化版：取前3档的总份额
    fn calculate_available_liquidity(&self, orderbook: &BookSnapshot) -> f64 {
        orderbook
            .asks
            .iter()
            .take(3) // 前3档
            .filter_map(|level| level.size.parse::<f64>().ok())
            .sum()
    }

    /// 记录模拟成交（仅模拟模式）
    async fn record_simulated_fill(
        &self,
        side: String,
        action: String,
        token_id: String,
        price: f64,
        size: f64,
        timestamp_ns: u64,
    ) {
        let mut fills = self.simulated_fills.write().await;
        fills.push(SimulatedFill {
            side,
            action,
            token_id,
            price,
            size,
            timestamp_ns,
        });
    }

    /// 获取模拟成交记录（用于测试验证）
    pub async fn get_simulated_fills(&self) -> Vec<SimulatedFill> {
        self.simulated_fills.read().await.clone()
    }

    /// 清空模拟成交记录（用于测试重置）
    pub async fn clear_simulated_fills(&self) {
        self.simulated_fills.write().await.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::polymarket::PriceLevel;
    use crate::polymarket_clob::{PolymarketAsyncClient, SharedAsyncClient};

    fn create_test_orderbook(total_size: f64) -> BookSnapshot {
        BookSnapshot {
            asset_id: "test_token".to_string(),
            bids: vec![],
            asks: vec![
                PriceLevel {
                    price: "0.50".to_string(),
                    size: (total_size / 3.0).to_string(),
                },
                PriceLevel {
                    price: "0.51".to_string(),
                    size: (total_size / 3.0).to_string(),
                },
                PriceLevel {
                    price: "0.52".to_string(),
                    size: (total_size / 3.0).to_string(),
                },
            ],
        }
    }

    fn create_test_client() -> Arc<SharedAsyncClient> {
        // 注意：这是一个占位符，真实测试需要 mock
        // 为了通过编译，我们创建一个实际的客户端（但不会真正调用 API）
        let inner = PolymarketAsyncClient::new(
            "https://clob.polymarket.com",
            137,
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            "0x0000000000000000000000000000000000000000",
        )
        .unwrap();

        let creds = serde_json::json!({
            "apiKey": "test",
            "secret": "dGVzdA==",  // base64 编码的 "test"
            "passphrase": "test"
        });
        let creds: crate::polymarket_clob::ApiCreds =
            serde_json::from_value(creds).unwrap();
        let prepared = crate::polymarket_clob::PreparedCreds::from_api_creds(&creds)
            .expect("Failed to create PreparedCreds");

        Arc::new(SharedAsyncClient::new(inner, prepared, 137))
    }

    #[tokio::test]
    async fn test_dry_run_mode() {
        let client = create_test_client();
        let engine = UpDownExecutionEngine::new(client, true, 10.0);

        assert!(engine.dry_run, "应启用模拟模式");
    }

    #[tokio::test]
    async fn test_liquidity_calculation() {
        let client = create_test_client();
        let engine = UpDownExecutionEngine::new(client, true, 10.0);

        let orderbook = create_test_orderbook(90.0); // 总共 90 份
        let liquidity = engine.calculate_available_liquidity(&orderbook);

        assert_eq!(liquidity, 90.0, "应计算出正确的流动性");
    }

    #[tokio::test]
    async fn test_insufficient_liquidity() {
        let client = create_test_client();
        let engine = UpDownExecutionEngine::new(client, true, 50.0); // 需要 50

        let orderbook = create_test_orderbook(30.0); // 只有 30
        let result = engine
            .execute_leg1_buy(
                DumpSide::Up,
                "token123",
                5000,
                50.0,
                &orderbook,
                1_000_000_000,
            )
            .await
            .unwrap();

        assert!(!result.success, "流动性不足时应失败");
        assert!(result.error_msg.is_some());
    }

    #[tokio::test]
    async fn test_simulated_fill_recording() {
        let client = create_test_client();
        let engine = UpDownExecutionEngine::new(client, true, 10.0);

        let orderbook = create_test_orderbook(100.0);
        let _ = engine
            .execute_leg1_buy(
                DumpSide::Up,
                "token123",
                5000,
                20.0,
                &orderbook,
                1_000_000_000,
            )
            .await;

        let fills = engine.get_simulated_fills().await;
        assert_eq!(fills.len(), 1, "应记录一条模拟成交");
        assert_eq!(fills[0].side, "Up");
        assert_eq!(fills[0].action, "Buy");
        assert_eq!(fills[0].size, 20.0);
    }

    #[tokio::test]
    async fn test_clear_simulated_fills() {
        let client = create_test_client();
        let engine = UpDownExecutionEngine::new(client, true, 10.0);

        engine
            .record_simulated_fill(
                "Up".to_string(),
                "Buy".to_string(),
                "token".to_string(),
                0.50,
                20.0,
                1_000_000_000,
            )
            .await;

        engine.clear_simulated_fills().await;

        let fills = engine.get_simulated_fills().await;
        assert_eq!(fills.len(), 0, "清空后应无记录");
    }
}
