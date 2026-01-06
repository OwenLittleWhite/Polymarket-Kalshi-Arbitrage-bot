use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

use crate::polymarket::GammaClient;

/// 单个轮次的信息
#[derive(Debug, Clone)]
pub struct RoundInfo {
    /// Polymarket 市场 slug（例如 "btc-updown-15m-1767276000"）
    pub slug: String,

    /// Up 侧 token ID
    pub up_token_id: String,

    /// Down 侧 token ID
    pub down_token_id: String,

    /// 轮次开始时间（Unix 时间戳，秒）
    pub start_timestamp: u64,

    /// 轮次结束时间（Unix 时间戳，秒）
    pub end_timestamp: u64,

    /// 资产类型（"BTC" | "ETH" | "SOL"）
    pub asset: String,
}

/// 轮次管理器
///
/// 负责：
/// - 生成 15 分钟轮次的 slug
/// - 从 Polymarket Gamma API 获取 token IDs
/// - 管理多资产（BTC/ETH/SOL）的当前轮次
/// - **预加载下一轮次**，实现零延迟切换
/// - 检测轮次是否结束
pub struct RoundManager {
    /// 当前活跃轮次（资产名 → RoundInfo）
    current_rounds: Arc<RwLock<HashMap<String, RoundInfo>>>,

    /// 下一轮次（预加载，资产名 → RoundInfo）
    next_rounds: Arc<RwLock<HashMap<String, RoundInfo>>>,

    /// Gamma API 客户端（复用现有代码）
    gamma_client: Arc<GammaClient>,

    /// 启用的资产列表（例如 ["BTC", "ETH", "SOL"]）
    enabled_assets: Vec<String>,
}

impl RoundManager {
    /// 创建新的轮次管理器
    ///
    /// # 参数
    /// - `enabled_assets`: 要监控的资产列表，例如 vec!["BTC".to_string(), "ETH".to_string()]
    pub fn new(enabled_assets: Vec<String>) -> Self {
        Self {
            current_rounds: Arc::new(RwLock::new(HashMap::new())),
            next_rounds: Arc::new(RwLock::new(HashMap::new())),
            gamma_client: Arc::new(GammaClient::new()),
            enabled_assets,
        }
    }

    /// 生成轮次 slug
    ///
    /// 格式：`{asset}-updown-15m-{start_timestamp}`
    ///
    /// # 示例
    /// ```
    /// let slug = RoundManager::generate_slug("BTC", 1767276000);
    /// assert_eq!(slug, "btc-updown-15m-1767276000");
    /// ```
    pub fn generate_slug(asset: &str, start_timestamp: u64) -> String {
        format!("{}-updown-15m-{}", asset.to_lowercase(), start_timestamp)
    }

    /// 计算当前所在的轮次边界（15 分钟对齐）
    ///
    /// # 返回
    /// - `start_timestamp`: 轮次开始时间（秒）
    /// - `end_timestamp`: 轮次结束时间（秒）
    pub fn get_current_round_boundaries() -> (u64, u64) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let round_duration = 15 * 60; // 15 分钟 = 900 秒
        let start_timestamp = (now / round_duration) * round_duration;
        let end_timestamp = start_timestamp + round_duration;

        (start_timestamp, end_timestamp)
    }

    /// 初始化所有启用资产的当前轮次和下一轮次
    ///
    /// 应在程序启动时调用，会重试直到找到市场，并预加载下一轮次实现零延迟切换
    pub async fn initialize_all_rounds(&self) -> Result<()> {
        for asset in &self.enabled_assets {
            // 重试机制：最多尝试 10 次，每次间隔 2 秒
            let mut attempts = 0;
            loop {
                match self.fetch_and_update_round(asset).await {
                    Ok(_) => {
                        tracing::info!("✅ {} 当前轮次初始化成功", asset);

                        // 预加载下一轮次（后台任务，失败不影响启动）
                        let asset_clone = asset.clone();
                        let self_clone = self.clone_for_background();
                        tokio::spawn(async move {
                            if let Err(e) = self_clone.preload_next_round(&asset_clone).await {
                                tracing::warn!("⚠️  预加载 {} 下一轮次失败: {}（稍后会重试）", asset_clone, e);
                            }
                        });

                        break;
                    }
                    Err(e) => {
                        attempts += 1;
                        if attempts >= 10 {
                            return Err(anyhow!("初始化 {} 失败（尝试 {} 次）: {}", asset, attempts, e));
                        }
                        tracing::warn!("⚠️  初始化 {} 失败（第 {}/10 次）: {}，2秒后重试...", asset, attempts, e);
                        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                    }
                }
            }
        }
        Ok(())
    }

    /// 克隆必要字段用于后台任务
    fn clone_for_background(&self) -> Self {
        Self {
            current_rounds: self.current_rounds.clone(),
            next_rounds: self.next_rounds.clone(),
            gamma_client: self.gamma_client.clone(),
            enabled_assets: self.enabled_assets.clone(),
        }
    }

    /// 预加载下一轮次（后台异步）
    async fn preload_next_round(&self, asset: &str) -> Result<()> {
        let (_, current_end_ts) = Self::get_current_round_boundaries();
        let next_start_ts = current_end_ts;
        let next_end_ts = next_start_ts + 900;

        let slug = Self::generate_slug(asset, next_start_ts);
        tracing::debug!("🔮 预加载下一轮次: {}", slug);

        let tokens = self.gamma_client.lookup_market(&slug).await?;

        let Some((up_token_id, down_token_id)) = tokens else {
            return Err(anyhow!("未找到下一轮次市场: {}", slug));
        };

        let next_round = RoundInfo {
            slug: slug.clone(),
            up_token_id,
            down_token_id,
            start_timestamp: next_start_ts,
            end_timestamp: next_end_ts,
            asset: asset.to_uppercase(),
        };

        let mut next_rounds = self.next_rounds.write().await;
        next_rounds.insert(asset.to_uppercase(), next_round);

        tracing::info!("✅ {} 下一轮次预加载成功: {}", asset, slug);
        Ok(())
    }

    /// 获取并更新指定资产的当前轮次
    ///
    /// 从 Gamma API 获取 token IDs
    async fn fetch_and_update_round(&self, asset: &str) -> Result<()> {
        let (start_ts, end_ts) = Self::get_current_round_boundaries();
        let slug = Self::generate_slug(asset, start_ts);

        tracing::debug!("🔍 查找市场: {} (时间戳: {})", slug, start_ts);

        // 调用现有的 GammaClient（复用代码）
        let tokens = self.gamma_client.lookup_market(&slug).await?;

        let Some((up_token_id, down_token_id)) = tokens else {
            return Err(anyhow!("未找到市场 slug: {} (可能市场尚未创建，或 slug 格式不对)", slug));
        };

        let round_info = RoundInfo {
            slug: slug.clone(),
            up_token_id,
            down_token_id,
            start_timestamp: start_ts,
            end_timestamp: end_ts,
            asset: asset.to_uppercase(),
        };

        let mut rounds = self.current_rounds.write().await;
        rounds.insert(asset.to_uppercase(), round_info);

        Ok(())
    }

    /// 获取指定资产的当前轮次信息
    ///
    /// # 返回
    /// - `Some(RoundInfo)` - 如果轮次已初始化
    /// - `None` - 如果轮次未初始化
    pub async fn get_round(&self, asset: &str) -> Option<RoundInfo> {
        let rounds = self.current_rounds.read().await;
        rounds.get(&asset.to_uppercase()).cloned()
    }

    /// 检查指定资产的轮次是否已结束
    ///
    /// # 返回
    /// - `true` - 当前时间 >= end_timestamp
    /// - `false` - 轮次仍在进行中
    pub async fn is_round_ended(&self, asset: &str) -> bool {
        let rounds = self.current_rounds.read().await;

        let Some(round) = rounds.get(&asset.to_uppercase()) else {
            return false;
        };

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        now >= round.end_timestamp
    }

    /// 获取轮次剩余秒数
    ///
    /// # 返回
    /// - `Some(seconds)` - 剩余秒数（如果轮次存在）
    /// - `None` - 轮次不存在
    pub async fn get_remaining_seconds(&self, asset: &str) -> Option<u64> {
        let rounds = self.current_rounds.read().await;

        let round = rounds.get(&asset.to_uppercase())?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        if now >= round.end_timestamp {
            Some(0)
        } else {
            Some(round.end_timestamp - now)
        }
    }

    /// 切换到下一个轮次（当当前轮次结束时调用）
    ///
    /// 使用预加载的下一轮次，实现**零延迟切换**
    ///
    /// # 返回
    /// - `Ok(RoundInfo)` - 新轮次信息
    /// - `Err` - 预加载的轮次不存在（不应该发生）
    pub async fn switch_to_next_round(&self, asset: &str) -> Result<RoundInfo> {
        tracing::info!("🔄 {} 轮次结束，切换到下一轮次...", asset);

        // 尝试从预加载的下一轮次中获取
        let next_round = {
            let mut next_rounds = self.next_rounds.write().await;
            next_rounds.remove(&asset.to_uppercase())
        };

        if let Some(round) = next_round {
            // 零延迟切换：直接使用预加载的轮次
            let mut current_rounds = self.current_rounds.write().await;
            current_rounds.insert(asset.to_uppercase(), round.clone());
            drop(current_rounds);

            tracing::info!("✅ {} 切换到新轮次: {} (预加载)", asset, round.slug);

            // 立即预加载再下一轮次
            let asset_clone = asset.to_string();
            let self_clone = self.clone_for_background();
            tokio::spawn(async move {
                if let Err(e) = self_clone.preload_next_round(&asset_clone).await {
                    tracing::warn!("⚠️  预加载 {} 再下一轮次失败: {}", asset_clone, e);
                }
            });

            return Ok(round);
        }

        // 备用方案：如果预加载失败，则实时查询（有延迟）
        tracing::warn!("⚠️  {} 下一轮次未预加载，使用实时查询（会有延迟）", asset);

        let mut attempts = 0;
        loop {
            match self.fetch_and_update_round(asset).await {
                Ok(_) => {
                    let round = self.get_round(asset)
                        .await
                        .ok_or_else(|| anyhow!("切换轮次后无法获取信息"))?;

                    tracing::info!("✅ {} 切换到新轮次: {} (实时查询)", asset, round.slug);

                    // 预加载下一轮次
                    let asset_clone = asset.to_string();
                    let self_clone = self.clone_for_background();
                    tokio::spawn(async move {
                        if let Err(e) = self_clone.preload_next_round(&asset_clone).await {
                            tracing::warn!("⚠️  预加载 {} 下一轮次失败: {}", asset_clone, e);
                        }
                    });

                    return Ok(round);
                }
                Err(e) => {
                    attempts += 1;
                    if attempts >= 10 {
                        return Err(anyhow!("切换 {} 轮次失败（尝试 {} 次）: {}", asset, attempts, e));
                    }
                    tracing::warn!("⚠️  切换 {} 轮次失败（第 {}/10 次）: {}，2秒后重试...", asset, attempts, e);
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_slug() {
        let slug = RoundManager::generate_slug("BTC", 1767276000);
        assert_eq!(slug, "btc-updown-15m-1767276000");

        let slug2 = RoundManager::generate_slug("ETH", 1767277800);
        assert_eq!(slug2, "eth-updown-15m-1767277800");
    }

    #[test]
    fn test_round_boundaries_alignment() {
        // 测试轮次边界对齐到 15 分钟
        let (start, end) = RoundManager::get_current_round_boundaries();

        assert_eq!(start % 900, 0, "开始时间应对齐到 15 分钟");
        assert_eq!(end - start, 900, "轮次持续时间应为 900 秒");
    }

    #[tokio::test]
    async fn test_manager_initialization() {
        let manager = RoundManager::new(vec!["BTC".to_string(), "ETH".to_string()]);

        assert_eq!(manager.enabled_assets.len(), 2);
        assert!(manager.enabled_assets.contains(&"BTC".to_string()));
    }

    #[tokio::test]
    async fn test_get_round_before_init() {
        let manager = RoundManager::new(vec!["BTC".to_string()]);

        let round = manager.get_round("BTC").await;
        assert!(round.is_none(), "初始化前应返回 None");
    }

    #[tokio::test]
    async fn test_is_round_ended_no_round() {
        let manager = RoundManager::new(vec!["BTC".to_string()]);

        let ended = manager.is_round_ended("BTC").await;
        assert!(!ended, "不存在的轮次应返回 false");
    }

    #[tokio::test]
    async fn test_get_remaining_seconds_no_round() {
        let manager = RoundManager::new(vec!["BTC".to_string()]);

        let remaining = manager.get_remaining_seconds("BTC").await;
        assert!(remaining.is_none(), "不存在的轮次应返回 None");
    }

    #[tokio::test]
    async fn test_manual_round_insertion_and_check() {
        // 手动插入一个轮次进行测试
        let manager = RoundManager::new(vec!["BTC".to_string()]);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let test_round = RoundInfo {
            slug: "btc-updown-15m-1767276000".to_string(),
            up_token_id: "token_up".to_string(),
            down_token_id: "token_down".to_string(),
            start_timestamp: now - 100,
            end_timestamp: now + 500, // 未结束
            asset: "BTC".to_string(),
        };

        {
            let mut rounds = manager.current_rounds.write().await;
            rounds.insert("BTC".to_string(), test_round.clone());
        }

        // 测试获取轮次
        let round = manager.get_round("BTC").await;
        assert!(round.is_some());
        assert_eq!(round.unwrap().slug, "btc-updown-15m-1767276000");

        // 测试轮次未结束
        let ended = manager.is_round_ended("BTC").await;
        assert!(!ended);

        // 测试剩余秒数
        let remaining = manager.get_remaining_seconds("BTC").await;
        assert!(remaining.is_some());
        assert!(remaining.unwrap() > 0 && remaining.unwrap() <= 500);
    }

    #[tokio::test]
    async fn test_round_ended() {
        let manager = RoundManager::new(vec!["BTC".to_string()]);

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        let expired_round = RoundInfo {
            slug: "btc-updown-15m-1767276000".to_string(),
            up_token_id: "token_up".to_string(),
            down_token_id: "token_down".to_string(),
            start_timestamp: now - 1000,
            end_timestamp: now - 100, // 已结束
            asset: "BTC".to_string(),
        };

        {
            let mut rounds = manager.current_rounds.write().await;
            rounds.insert("BTC".to_string(), expired_round);
        }

        // 测试轮次已结束
        let ended = manager.is_round_ended("BTC").await;
        assert!(ended);

        // 测试剩余秒数为 0
        let remaining = manager.get_remaining_seconds("BTC").await;
        assert_eq!(remaining, Some(0));
    }
}
