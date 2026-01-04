use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tracing::info;

/// 轮次快照数据
///
/// 文件命名格式: `{ASSET}_{timestamp}.json`
/// 例如: `BTC_20260101101500.json`
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoundSnapshot {
    /// 资产类型（BTC/ETH/SOL）
    pub asset: String,

    /// 轮次 slug
    pub slug: String,

    /// Up token ID
    pub up_token_id: String,

    /// Down token ID
    pub down_token_id: String,

    /// 轮次开始时间（Unix 秒）
    pub start_timestamp: u64,

    /// 轮次结束时间（Unix 秒）
    pub end_timestamp: u64,

    /// 交易记录（模拟或真实）
    pub trades: Vec<TradeRecord>,
}

/// 价格快照（来自 WebSocket）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceSnapshot {
    /// 时间戳（Unix 纳秒）
    pub timestamp_ns: u64,

    /// Up 侧最优 ask 价格（BPS）
    pub up_ask: u16,

    /// Down 侧最优 ask 价格（BPS）
    pub down_ask: u16,

    /// Up 侧流动性（前3档总份额）
    pub up_liquidity: f64,

    /// Down 侧流动性（前3档总份额）
    pub down_liquidity: f64,
}

/// 交易记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TradeRecord {
    /// 交易时间戳（Unix 纳秒）
    pub timestamp_ns: u64,

    /// 交易类型（"buy" | "sell"）
    pub action: String,

    /// 交易侧（"up" | "down"）
    pub side: String,

    /// Token ID
    pub token_id: String,

    /// 价格（0.0-1.0）
    pub price: f64,

    /// 份额
    pub size: f64,

    /// 是否为模拟交易
    pub is_simulated: bool,
}

/// 数据记录器
pub struct DataLogger {
    /// 数据目录
    data_dir: PathBuf,
}

impl DataLogger {
    /// 创建新的数据记录器
    ///
    /// # 参数
    /// - `data_dir`: 数据存储目录（例如 "./data"）
    pub fn new<P: AsRef<Path>>(data_dir: P) -> Self {
        Self {
            data_dir: data_dir.as_ref().to_path_buf(),
        }
    }

    /// 生成轮次文件名
    ///
    /// 格式: `{ASSET}_{YYYYMMDDHHMMSS}.json`
    ///
    /// # 示例
    /// ```
    /// // start_timestamp = 1767262500 (2025-01-01 10:15:00 UTC)
    /// // 输出: "BTC_20250101101500.json"
    /// ```
    pub fn generate_filename(asset: &str, start_timestamp: u64) -> String {
        use chrono::{TimeZone, Utc};

        let dt = Utc.timestamp_opt(start_timestamp as i64, 0).unwrap();
        let formatted = dt.format("%Y%m%d%H%M%S").to_string();

        format!("{}_{}.json", asset.to_uppercase(), formatted)
    }

    /// 创建新的轮次快照
    pub fn create_round_snapshot(
        asset: String,
        slug: String,
        up_token_id: String,
        down_token_id: String,
        start_timestamp: u64,
        end_timestamp: u64,
    ) -> RoundSnapshot {
        RoundSnapshot {
            asset,
            slug,
            up_token_id,
            down_token_id,
            start_timestamp,
            end_timestamp,
            trades: Vec::new(),
        }
    }

    /// 保存轮次快照到文件
    ///
    /// 如果文件已存在，会覆盖（用于更新数据）
    pub async fn save_snapshot(&self, snapshot: &RoundSnapshot) -> Result<PathBuf> {
        // 确保数据目录存在
        fs::create_dir_all(&self.data_dir).await?;

        // 生成文件路径
        let filename = Self::generate_filename(&snapshot.asset, snapshot.start_timestamp);
        let file_path = self.data_dir.join(&filename);

        // 序列化为 JSON（格式化输出）
        let json = serde_json::to_string_pretty(snapshot)?;

        // 写入文件
        let mut file = fs::File::create(&file_path).await?;
        file.write_all(json.as_bytes()).await?;

        info!("💾 保存数据: {} ({} 笔交易)",
              filename, snapshot.trades.len());

        Ok(file_path)
    }

    /// 加载轮次快照从文件
    pub async fn load_snapshot(&self, asset: &str, start_timestamp: u64) -> Result<RoundSnapshot> {
        let filename = Self::generate_filename(asset, start_timestamp);
        let file_path = self.data_dir.join(&filename);

        let content = fs::read_to_string(&file_path).await?;
        let snapshot: RoundSnapshot = serde_json::from_str(&content)?;

        Ok(snapshot)
    }

    /// 检查轮次文件是否存在
    pub async fn snapshot_exists(&self, asset: &str, start_timestamp: u64) -> bool {
        let filename = Self::generate_filename(asset, start_timestamp);
        let file_path = self.data_dir.join(&filename);

        file_path.exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_filename() {
        // 2026-01-01 10:15:00 UTC
        let timestamp = 1767262500;
        let filename = DataLogger::generate_filename("BTC", timestamp);

        assert_eq!(filename, "BTC_20260101101500.json");
    }

    #[test]
    fn test_generate_filename_different_assets() {
        let timestamp = 1767262500;

        assert_eq!(
            DataLogger::generate_filename("btc", timestamp),
            "BTC_20260101101500.json"
        );
        assert_eq!(
            DataLogger::generate_filename("ETH", timestamp),
            "ETH_20260101101500.json"
        );
        assert_eq!(
            DataLogger::generate_filename("sol", timestamp),
            "SOL_20260101101500.json"
        );
    }

    #[test]
    fn test_create_round_snapshot() {
        let snapshot = DataLogger::create_round_snapshot(
            "BTC".to_string(),
            "btc-updown-15m-1767262500".to_string(),
            "token_up".to_string(),
            "token_down".to_string(),
            1767262500,
            1767263400,
        );

        assert_eq!(snapshot.asset, "BTC");
        assert_eq!(snapshot.slug, "btc-updown-15m-1767262500");
        assert_eq!(snapshot.trades.len(), 0);
    }

    #[tokio::test]
    async fn test_save_and_load_snapshot() {
        let temp_dir = std::env::temp_dir().join("test_data_logger");
        let logger = DataLogger::new(&temp_dir);

        let mut snapshot = DataLogger::create_round_snapshot(
            "BTC".to_string(),
            "btc-updown-15m-1767262500".to_string(),
            "token_up".to_string(),
            "token_down".to_string(),
            1767262500,
            1767263400,
        );

        // 添加测试数据
        snapshot.trades.push(TradeRecord {
            timestamp_ns: 2_000_000_000,
            action: "buy".to_string(),
            side: "up".to_string(),
            token_id: "token_up".to_string(),
            price: 0.50,
            size: 20.0,
            is_simulated: true,
        });

        // 保存
        let file_path = logger.save_snapshot(&snapshot).await.unwrap();
        assert!(file_path.exists());

        // 加载
        let loaded = logger.load_snapshot("BTC", 1767262500).await.unwrap();

        assert_eq!(loaded.asset, "BTC");
        assert_eq!(loaded.trades.len(), 1);
        assert_eq!(loaded.trades[0].size, 20.0);

        // 清理
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_snapshot_exists() {
        let temp_dir = std::env::temp_dir().join("test_data_logger_exists");
        let logger = DataLogger::new(&temp_dir);

        assert!(!logger.snapshot_exists("BTC", 1767262500).await);

        let snapshot = DataLogger::create_round_snapshot(
            "BTC".to_string(),
            "btc-updown-15m-1767262500".to_string(),
            "token_up".to_string(),
            "token_down".to_string(),
            1767262500,
            1767263400,
        );

        logger.save_snapshot(&snapshot).await.unwrap();

        assert!(logger.snapshot_exists("BTC", 1767262500).await);

        // 清理
        std::fs::remove_dir_all(&temp_dir).ok();
    }

    #[tokio::test]
    async fn test_overwrite_existing_file() {
        let temp_dir = std::env::temp_dir().join("test_data_logger_overwrite");
        let logger = DataLogger::new(&temp_dir);

        let mut snapshot = DataLogger::create_round_snapshot(
            "BTC".to_string(),
            "btc-updown-15m-1767262500".to_string(),
            "token_up".to_string(),
            "token_down".to_string(),
            1767262500,
            1767263400,
        );

        // 第一次保存
        logger.save_snapshot(&snapshot).await.unwrap();

        // 添加数据后再次保存（覆盖）
        snapshot.trades.push(TradeRecord {
            timestamp_ns: 1_000_000_000,
            action: "buy".to_string(),
            side: "up".to_string(),
            token_id: "token_up".to_string(),
            price: 0.50,
            size: 10.0,
            is_simulated: true,
        });

        logger.save_snapshot(&snapshot).await.unwrap();

        // 加载验证
        let loaded = logger.load_snapshot("BTC", 1767262500).await.unwrap();
        assert_eq!(loaded.trades.len(), 1);

        // 清理
        std::fs::remove_dir_all(&temp_dir).ok();
    }
}
