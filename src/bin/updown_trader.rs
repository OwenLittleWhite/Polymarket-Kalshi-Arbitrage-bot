//! Up/Down 15分钟交易策略主程序
//!
//! 运行方式：
//! ```bash
//! # 模拟模式
//! DRY_RUN=1 POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... cargo run --bin updown_trader
//!
//! # 真实交易
//! DRY_RUN=0 POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... cargo run --bin updown_trader
//! ```

use anyhow::{Result, Context};
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{info, warn, error};

use prediction_market_arbitrage::{
    data_logger::{DataLogger, RoundSnapshot, TradeRecord},
    dump_detector::{DumpDetector, DumpSide},
    polymarket::BookSnapshot,
    polymarket_clob::{PolymarketAsyncClient, PreparedCreds, SharedAsyncClient},
    round_manager::{RoundInfo, RoundManager},
    strategy_state::{StrategyState, StrategyStateMachine},
    updown_execution::UpDownExecutionEngine,
};
use serde_json;

const POLYMARKET_WS_URL: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";

/// 配置参数
#[derive(Clone)]
struct Config {
    // Polymarket 凭证
    poly_private_key: String,
    poly_funder: String,
    poly_signature_type: i32,  // 0=EOA, 1=Gnosis Safe, 2=Poly Proxy

    // 策略参数
    position_size_shares: f64,
    strategy_sum_target: f64,
    strategy_move_pct: f64,
    strategy_window_ns: u64,

    // 风险管理
    stop_loss_enabled: bool,
    stop_loss_pct: f64,
    early_exit_enabled: bool,
    early_exit_profit_pct: f64,

    // 多资产
    enabled_assets: Vec<String>,

    // 系统
    dry_run: bool,
    data_dir: String,
}

impl Config {
    fn from_env() -> Result<Self> {
        Ok(Self {
            poly_private_key: std::env::var("POLY_PRIVATE_KEY")
                .context("缺少 POLY_PRIVATE_KEY 环境变量")?,
            poly_funder: std::env::var("POLY_FUNDER")
                .context("缺少 POLY_FUNDER 环境变量")?,
            poly_signature_type: std::env::var("POLY_SIGNATURE_TYPE")
                .unwrap_or_else(|_| "0".to_string())
                .parse()
                .context("POLY_SIGNATURE_TYPE 必须是 0/1/2")?,

            position_size_shares: std::env::var("POSITION_SIZE_SHARES")
                .unwrap_or_else(|_| "20".to_string())
                .parse()?,

            strategy_sum_target: std::env::var("STRATEGY_SUM_TARGET")
                .unwrap_or_else(|_| "0.95".to_string())
                .parse()?,

            strategy_move_pct: std::env::var("STRATEGY_MOVE_PCT")
                .unwrap_or_else(|_| "0.15".to_string())
                .parse()?,

            strategy_window_ns: std::env::var("STRATEGY_WINDOW_SECS")
                .unwrap_or_else(|_| "3".to_string())
                .parse::<u64>()?
                * 1_000_000_000,

            stop_loss_enabled: std::env::var("STOP_LOSS_ENABLED")
                .unwrap_or_else(|_| "true".to_string())
                == "true",

            stop_loss_pct: std::env::var("STOP_LOSS_PCT")
                .unwrap_or_else(|_| "0.50".to_string())
                .parse()?,

            early_exit_enabled: std::env::var("EARLY_EXIT_ENABLED")
                .unwrap_or_else(|_| "false".to_string())
                == "true",

            early_exit_profit_pct: std::env::var("EARLY_EXIT_PROFIT_PCT")
                .unwrap_or_else(|_| "0.10".to_string())
                .parse()?,

            enabled_assets: std::env::var("ENABLED_ASSETS")
                .unwrap_or_else(|_| "BTC".to_string())
                .split(',')
                .map(|s| s.trim().to_uppercase())
                .collect(),

            dry_run: std::env::var("DRY_RUN")
                .map(|v| v == "1" || v == "true")
                .unwrap_or(true),

            data_dir: std::env::var("DATA_DIR")
                .unwrap_or_else(|_| "./data".to_string()),
        })
    }
}

/// 单个资产的交易上下文
struct AssetTrader {
    asset: String,
    config: Config,
    round_manager: Arc<RoundManager>,
    dump_detector: Arc<RwLock<DumpDetector>>,
    strategy_state: Arc<StrategyStateMachine>,
    executor: Arc<UpDownExecutionEngine>,
    data_logger: Arc<DataLogger>,
    current_snapshot: Arc<RwLock<Option<RoundSnapshot>>>,
}

impl AssetTrader {
    async fn new(
        asset: String,
        config: Config,
        round_manager: Arc<RoundManager>,
        executor: Arc<UpDownExecutionEngine>,
        data_logger: Arc<DataLogger>,
    ) -> Self {
        let dump_detector = Arc::new(RwLock::new(DumpDetector::new(
            config.strategy_window_ns,
            config.strategy_move_pct,
        )));

        let strategy_state = Arc::new(StrategyStateMachine::new(
            config.strategy_sum_target,
            if config.early_exit_enabled {
                Some(config.early_exit_profit_pct)
            } else {
                None
            },
            if config.stop_loss_enabled {
                Some(config.stop_loss_pct)
            } else {
                None
            },
        ));

        Self {
            asset,
            config,
            round_manager,
            dump_detector,
            strategy_state,
            executor,
            data_logger,
            current_snapshot: Arc::new(RwLock::new(None)),
        }
    }

    /// 处理价格更新
    async fn on_price_update(
        &self,
        up_ask: u16,
        down_ask: u16,
        orderbook: &BookSnapshot,
    ) -> Result<()> {
        let now_ns = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_nanos() as u64;

        // 获取当前轮次
        let Some(round) = self.round_manager.get_round(&self.asset).await else {
            return Ok(());
        };

        let state = self.strategy_state.get_state().await;

        // Priority 0: 检查轮次是否结束
        if self.round_manager.is_round_ended(&self.asset).await {
            self.handle_round_end(&round).await?;
            return Ok(());
        }

        // Priority 1: 早期退出检查（价格反弹）
        if state == StrategyState::WaitingForHedge {
            let ctx = self.strategy_state.get_context().await;
            let current_side = ctx.leg1_side.unwrap();
            let current_price = match current_side {
                DumpSide::Up => up_ask,
                DumpSide::Down => down_ask,
            };

            if self.strategy_state.check_early_exit(current_price).await {
                info!("💰 价格反弹，早期退出");
                self.handle_exit(&round, "early_exit", current_price, orderbook, now_ns)
                    .await?;
                return Ok(());
            }

            // Priority 2: 强制平仓检查（< 30秒剩余）
            if let Some(remaining_secs) = self.round_manager.get_remaining_seconds(&self.asset).await {
                if remaining_secs < 30 {
                    warn!("⏰ 轮次即将结束（剩余 {} 秒），强制平仓", remaining_secs);
                    self.handle_exit(&round, "force_close", current_price, orderbook, now_ns)
                        .await?;
                    return Ok(());
                }
            }

            // Priority 3: 止损检查
            if self.strategy_state.check_stop_loss(current_price).await {
                warn!("🛑 触发止损（亏损 >= {}%）", self.config.stop_loss_pct * 100.0);
                self.handle_exit(&round, "stop_loss", current_price, orderbook, now_ns)
                    .await?;
                return Ok(());
            }

            // Priority 4: 对冲条件检查
            if self.strategy_state.check_hedge_condition(up_ask, down_ask).await {
                info!("✅ 对冲条件满足");
                self.handle_hedge(&round, up_ask, down_ask, orderbook, now_ns)
                    .await?;
                return Ok(());
            }
        }

        // Priority 5: 抛售检测（仅在 WaitingForDump 状态）
        if state == StrategyState::WaitingForDump {
            let detector = self.dump_detector.write().await;
            if let Some(side) = detector.on_price_update(up_ask, down_ask, now_ns).await {
                drop(detector);
                info!("🔔 检测到抛售: {:?} 侧", side);
                self.handle_dump(&round, side, orderbook, now_ns).await?;
            }
        }

        Ok(())
    }

    /// 处理抛售事件（Phase 1）
    async fn handle_dump(
        &self,
        round: &RoundInfo,
        side: DumpSide,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<()> {
        let (token_id, price) = match side {
            DumpSide::Up => (&round.up_token_id, orderbook.asks[0].price.parse::<f64>()? * 10000.0),
            DumpSide::Down => (&round.down_token_id, orderbook.asks[0].price.parse::<f64>()? * 10000.0),
        };

        let result = self
            .executor
            .execute_leg1_buy(
                side,
                token_id,
                price as u16,
                self.config.position_size_shares,
                orderbook,
                timestamp_ns,
            )
            .await?;

        if result.success {
            info!("✅ Phase 1 买入成功: {:.2} 份 @ ${:.4}", result.filled_size, result.avg_price);

            self.strategy_state
                .record_leg1_execution(
                    side,
                    price as u16,
                    result.filled_size,
                    token_id.to_string(),
                    timestamp_ns,
                )
                .await;

            // 记录交易
            self.record_trade(timestamp_ns, "buy", side, token_id, result.avg_price, result.filled_size)
                .await?;
        } else {
            warn!("❌ Phase 1 买入失败: {:?}", result.error_msg);
        }

        Ok(())
    }

    /// 处理对冲（Phase 2）
    async fn handle_hedge(
        &self,
        round: &RoundInfo,
        up_ask: u16,
        down_ask: u16,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<()> {
        let ctx = self.strategy_state.get_context().await;
        let leg1_side = ctx.leg1_side.unwrap();
        let leg1_token = ctx.leg1_token_id.unwrap();

        let (opposite_token, opposite_price) = match leg1_side {
            DumpSide::Up => (&round.down_token_id, down_ask),
            DumpSide::Down => (&round.up_token_id, up_ask),
        };

        let result = self
            .executor
            .execute_leg2_or_exit(
                "hedge",
                leg1_side,
                &leg1_token,
                Some(opposite_token),
                opposite_price,
                ctx.leg1_shares.unwrap(),
                orderbook,
                timestamp_ns,
            )
            .await?;

        if result.success {
            info!("✅ Phase 2 对冲成功: {:.2} 份 @ ${:.4}", result.filled_size, result.avg_price);

            let opposite_side = match leg1_side {
                DumpSide::Up => DumpSide::Down,
                DumpSide::Down => DumpSide::Up,
            };

            self.record_trade(timestamp_ns, "buy", opposite_side, opposite_token, result.avg_price, result.filled_size)
                .await?;

            self.strategy_state.mark_completed().await;
        } else {
            warn!("❌ Phase 2 对冲失败: {:?}", result.error_msg);
        }

        Ok(())
    }

    /// 处理退出（早期退出、止损、强制平仓）
    async fn handle_exit(
        &self,
        _round: &RoundInfo,
        reason: &str,
        current_price: u16,
        orderbook: &BookSnapshot,
        timestamp_ns: u64,
    ) -> Result<()> {
        let ctx = self.strategy_state.get_context().await;
        let leg1_side = ctx.leg1_side.unwrap();
        let leg1_token = ctx.leg1_token_id.unwrap();

        let result = self
            .executor
            .execute_leg2_or_exit(
                reason,
                leg1_side,
                &leg1_token,
                None,
                current_price,
                ctx.leg1_shares.unwrap(),
                orderbook,
                timestamp_ns,
            )
            .await?;

        if result.success {
            info!("✅ {} 成功: {:.2} 份 @ ${:.4}", reason, result.filled_size, result.avg_price);

            self.record_trade(timestamp_ns, "sell", leg1_side, &leg1_token, result.avg_price, result.filled_size)
                .await?;

            self.strategy_state.mark_completed().await;
        } else {
            warn!("❌ {} 失败: {:?}", reason, result.error_msg);
        }

        Ok(())
    }

    /// 处理轮次结束
    async fn handle_round_end(&self, round: &RoundInfo) -> Result<()> {
        info!("🔄 轮次结束: {}", round.slug);

        // 保存当前轮次数据
        if let Some(snapshot) = self.current_snapshot.read().await.as_ref() {
            self.data_logger.save_snapshot(snapshot).await?;
        }

        // 重置状态
        self.strategy_state.reset().await;
        let detector = self.dump_detector.write().await;
        detector.reset().await;
        drop(detector);

        // 切换到下一轮
        let new_round = self.round_manager.switch_to_next_round(&self.asset).await?;
        info!("✅ 切换到新轮次: {}", new_round.slug);

        // 创建新的快照
        let new_snapshot = DataLogger::create_round_snapshot(
            new_round.asset.clone(),
            new_round.slug.clone(),
            new_round.up_token_id.clone(),
            new_round.down_token_id.clone(),
            new_round.start_timestamp,
            new_round.end_timestamp,
        );

        *self.current_snapshot.write().await = Some(new_snapshot);

        Ok(())
    }

    /// 记录交易
    async fn record_trade(
        &self,
        timestamp_ns: u64,
        action: &str,
        side: DumpSide,
        token_id: &str,
        price: f64,
        size: f64,
    ) -> Result<()> {
        let mut snapshot_lock = self.current_snapshot.write().await;
        if let Some(snapshot) = snapshot_lock.as_mut() {
            snapshot.trades.push(TradeRecord {
                timestamp_ns,
                action: action.to_string(),
                side: match side {
                    DumpSide::Up => "up".to_string(),
                    DumpSide::Down => "down".to_string(),
                },
                token_id: token_id.to_string(),
                price,
                size,
                is_simulated: self.config.dry_run,
            });

            // 交易后立即保存
            self.data_logger.save_snapshot(snapshot).await?;
        }
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt()
        .with_target(false)
        .with_thread_ids(false)
        .init();

    // 加载配置
    let config = Config::from_env()?;

    info!("🚀 Up/Down 交易策略启动");
    info!("⚙️  模式: {}", if config.dry_run { "模拟（真实数据 + 假下单）" } else { "真实交易" });
    info!("⚙️  资产: {:?}", config.enabled_assets);
    info!("⚙️  仓位大小: {} 份", config.position_size_shares);
    info!("⚙️  对冲目标: {:.2}", config.strategy_sum_target);
    info!("⚙️  抛售阈值: {:.0}%", config.strategy_move_pct * 100.0);
    info!("⚙️  止损阈值: {:.0}%", config.stop_loss_pct * 100.0);
    info!("⚙️  签名类型: {} (0=EOA, 1=Safe, 2=Proxy)", config.poly_signature_type);

    // 初始化 Polymarket 客户端
    let poly_client = PolymarketAsyncClient::new_with_signature_type(
        "https://clob.polymarket.com",
        137,
        &config.poly_private_key,
        &config.poly_funder,
        config.poly_signature_type,
    )?;

    // 使用 nonce = 0（与官方 SDK 默认值一致）
    let api_creds = poly_client.derive_api_key(0).await?;
    let prepared_creds = PreparedCreds::from_api_creds(&api_creds)?;

    let poly_async = Arc::new(SharedAsyncClient::new(poly_client, prepared_creds, 137));

    info!("✅ Polymarket 客户端初始化完成");

    // 初始化共享组件
    let round_manager = Arc::new(RoundManager::new(config.enabled_assets.clone()));
    let executor = Arc::new(UpDownExecutionEngine::new(
        poly_async.clone(),
        config.dry_run,
        config.position_size_shares,
    ));
    let data_logger = Arc::new(DataLogger::new(&config.data_dir));

    // 初始化所有轮次
    round_manager.initialize_all_rounds().await?;

    for asset in &config.enabled_assets {
        if let Some(round) = round_manager.get_round(asset).await {
            info!("📊 当前轮次: {}", round.slug);
            info!("    Up Token: {}", round.up_token_id);
            info!("    Down Token: {}", round.down_token_id);
        }
    }

    // 创建资产交易器
    let mut traders: HashMap<String, AssetTrader> = HashMap::new();
    for asset in &config.enabled_assets {
        let trader = AssetTrader::new(
            asset.clone(),
            config.clone(),
            round_manager.clone(),
            executor.clone(),
            data_logger.clone(),
        )
        .await;

        // 初始化快照
        if let Some(round) = round_manager.get_round(asset).await {
            let snapshot = DataLogger::create_round_snapshot(
                round.asset.clone(),
                round.slug.clone(),
                round.up_token_id.clone(),
                round.down_token_id.clone(),
                round.start_timestamp,
                round.end_timestamp,
            );
            *trader.current_snapshot.write().await = Some(snapshot);
        }

        traders.insert(asset.clone(), trader);
    }

    info!("✅ 策略模块初始化完成");

    // 连接 WebSocket
    info!("🔌 连接 Polymarket WebSocket...");

    let (ws_stream, _) = connect_async(POLYMARKET_WS_URL).await
        .context("WebSocket 连接失败")?;

    let (mut write, mut read) = ws_stream.split();

    // 收集所有需要订阅的 token ID
    let mut all_token_ids = Vec::new();
    for asset in &config.enabled_assets {
        if let Some(round) = round_manager.get_round(asset).await {
            all_token_ids.push(round.up_token_id.clone());
            all_token_ids.push(round.down_token_id.clone());
            info!("📋 准备订阅 {}: {} / {}", asset, round.up_token_id, round.down_token_id);
        }
    }

    // 发送订阅消息（参考 src/polymarket.rs 的正确格式）
    if !all_token_ids.is_empty() {
        let subscribe_msg = serde_json::json!({
            "assets_ids": all_token_ids,
            "type": "market"
        });

        write.send(Message::Text(subscribe_msg.to_string())).await?;
        info!("✅ 已发送订阅请求，共 {} 个 token", all_token_ids.len());
    }

    info!("✅ WebSocket 连接成功，开始监听价格...");

    // 维护每个 token 的最新价格（token_id -> (price_bps, snapshot)）
    let mut token_prices: HashMap<String, (u16, BookSnapshot)> = HashMap::new();

    // 主循环：处理 WebSocket 消息
    while let Some(msg) = read.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(e) => {
                error!("WebSocket 错误: {}", e);
                continue;
            }
        };

        if let Message::Text(text) = msg {
            // 正确的格式：WebSocket 返回的是订单簿数组
            if let Ok(snapshots) = serde_json::from_str::<Vec<BookSnapshot>>(&text) {
                // 更新每个 token 的价格
                for snapshot in snapshots {
                    if !snapshot.asks.is_empty() {
                        if let Ok(price_f64) = snapshot.asks[0].price.parse::<f64>() {
                            let price_bps = (price_f64 * 10000.0) as u16;
                            token_prices.insert(snapshot.asset_id.clone(), (price_bps, snapshot));
                        }
                    }
                }

                // 对每个资产，检查是否同时有 up 和 down 的价格
                for (asset, trader) in &traders {
                    if let Some(round) = round_manager.get_round(asset).await {
                        // 获取 up 和 down 的真实价格
                        if let (Some((up_ask, up_snapshot)), Some((down_ask, _down_snapshot))) = (
                            token_prices.get(&round.up_token_id),
                            token_prices.get(&round.down_token_id),
                        ) {
                            // 使用 up 侧的订单簿快照（两个都可以，选一个即可）
                            if let Err(e) = trader
                                .on_price_update(*up_ask, *down_ask, up_snapshot)
                                .await
                            {
                                error!("处理价格更新失败 ({}): {}", asset, e);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}


