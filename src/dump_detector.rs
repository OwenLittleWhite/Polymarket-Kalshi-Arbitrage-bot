use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::Mutex;

/// 价格快照 - 用于检测价格变动
#[derive(Debug, Clone, Copy)]
pub struct PriceSnapshot {
    pub price: u16,       // BPS (0-10000, 代表 0.00-1.00)
    pub timestamp_ns: u64, // 纳秒时间戳
}

/// 抛售检测器 - 事件驱动设计
///
/// **关键设计**：
/// - 不使用死循环轮询
/// - 每次价格更新时调用 `on_price_update()`
/// - 维护滑动窗口内的价格历史
/// - 检测短时间内的大幅下跌
pub struct DumpDetector {
    /// Up 侧价格历史（滑动窗口）
    price_history_up: Arc<Mutex<VecDeque<PriceSnapshot>>>,

    /// Down 侧价格历史（滑动窗口）
    price_history_down: Arc<Mutex<VecDeque<PriceSnapshot>>>,

    /// 检测窗口大小（纳秒），默认 3 秒
    detection_window_ns: u64,

    /// 抛售阈值（百分比），例如 0.15 = 15%
    move_threshold: f64,
}

/// 抛售事件
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DumpSide {
    Up,
    Down,
}

impl DumpDetector {
    /// 创建新的抛售检测器
    ///
    /// # 参数
    /// - `window_ns`: 检测窗口大小（纳秒），推荐 3_000_000_000 (3秒)
    /// - `move_threshold`: 抛售阈值，例如 0.15 表示 15% 的下跌
    pub fn new(window_ns: u64, move_threshold: f64) -> Self {
        Self {
            price_history_up: Arc::new(Mutex::new(VecDeque::new())),
            price_history_down: Arc::new(Mutex::new(VecDeque::new())),
            detection_window_ns: window_ns,
            move_threshold,
        }
    }

    /// **核心方法 - 事件驱动**
    ///
    /// 当收到 WebSocket 价格更新时调用此方法
    ///
    /// # 返回
    /// - `Some(DumpSide::Up)` - Up 侧发生抛售
    /// - `Some(DumpSide::Down)` - Down 侧发生抛售
    /// - `None` - 无抛售
    pub async fn on_price_update(
        &self,
        up_price: u16,
        down_price: u16,
        timestamp_ns: u64,
    ) -> Option<DumpSide> {
        // 1. 添加新快照到历史
        let mut up_hist = self.price_history_up.lock().await;
        let mut down_hist = self.price_history_down.lock().await;

        up_hist.push_back(PriceSnapshot {
            price: up_price,
            timestamp_ns,
        });
        down_hist.push_back(PriceSnapshot {
            price: down_price,
            timestamp_ns,
        });

        // 2. 清理过期数据（超出窗口的快照）
        let cutoff_time = timestamp_ns.saturating_sub(self.detection_window_ns);

        while let Some(snapshot) = up_hist.front() {
            if snapshot.timestamp_ns < cutoff_time {
                up_hist.pop_front();
            } else {
                break;
            }
        }

        while let Some(snapshot) = down_hist.front() {
            if snapshot.timestamp_ns < cutoff_time {
                down_hist.pop_front();
            } else {
                break;
            }
        }

        // 3. 检测抛售（窗口内价格最高点到当前的跌幅）
        let dump_up = Self::check_dump(&up_hist, self.move_threshold);
        let dump_down = Self::check_dump(&down_hist, self.move_threshold);

        drop(up_hist);
        drop(down_hist);

        // 4. 返回检测结果（优先返回 Up 侧，因为通常只需要检测一侧）
        if dump_up {
            Some(DumpSide::Up)
        } else if dump_down {
            Some(DumpSide::Down)
        } else {
            None
        }
    }

    /// 检测单侧是否发生抛售
    ///
    /// 逻辑：窗口内最高价 → 当前价 的跌幅 >= move_threshold
    fn check_dump(history: &VecDeque<PriceSnapshot>, threshold: f64) -> bool {
        if history.len() < 2 {
            return false; // 数据不足
        }

        // 找到窗口内的最高价
        let max_price = history.iter().map(|s| s.price).max().unwrap_or(0);

        // 当前价格（最新快照）
        let current_price = history.back().unwrap().price;

        if max_price == 0 {
            return false;
        }

        // 计算跌幅百分比
        let drop_pct = (max_price as f64 - current_price as f64) / max_price as f64;

        drop_pct >= threshold
    }

    /// 重置检测器（用于轮次切换时）
    pub async fn reset(&self) {
        let mut up_hist = self.price_history_up.lock().await;
        let mut down_hist = self.price_history_down.lock().await;

        up_hist.clear();
        down_hist.clear();
    }

    /// 获取当前窗口内的快照数量（用于调试）
    pub async fn get_window_size(&self) -> (usize, usize) {
        let up_hist = self.price_history_up.lock().await;
        let down_hist = self.price_history_down.lock().await;
        (up_hist.len(), down_hist.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 辅助函数：将价格从 f64 (0.0-1.0) 转换为 BPS (0-10000)
    fn price_to_bps(price: f64) -> u16 {
        (price * 10000.0) as u16
    }

    #[tokio::test]
    async fn test_no_dump_stable_price() {
        // 测试：价格稳定，无抛售
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        let result = detector.on_price_update(
            price_to_bps(0.50),
            price_to_bps(0.50),
            1000,
        ).await;

        assert_eq!(result, None, "稳定价格不应触发抛售");
    }

    #[tokio::test]
    async fn test_dump_detected_up_side() {
        // 测试：Up 侧价格从 0.60 → 0.50（下跌 16.7%）
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        // 第一个快照：Up=0.60, Down=0.40
        detector.on_price_update(
            price_to_bps(0.60),
            price_to_bps(0.40),
            1_000_000_000,
        ).await;

        // 第二个快照（2秒后）：Up=0.50, Down=0.40（Up 跌 16.7%）
        let result = detector.on_price_update(
            price_to_bps(0.50),
            price_to_bps(0.40),
            3_000_000_000,
        ).await;

        assert_eq!(result, Some(DumpSide::Up), "Up 侧应检测到抛售");
    }

    #[tokio::test]
    async fn test_dump_detected_down_side() {
        // 测试：Down 侧价格从 0.70 → 0.59（下跌 15.7%）
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        detector.on_price_update(
            price_to_bps(0.30),
            price_to_bps(0.70),
            1_000_000_000,
        ).await;

        let result = detector.on_price_update(
            price_to_bps(0.30),
            price_to_bps(0.59),
            3_000_000_000,
        ).await;

        assert_eq!(result, Some(DumpSide::Down), "Down 侧应检测到抛售");
    }

    #[tokio::test]
    async fn test_no_dump_insufficient_drop() {
        // 测试：价格下跌 10%，未达到 15% 阈值
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        detector.on_price_update(
            price_to_bps(0.60),
            price_to_bps(0.40),
            1_000_000_000,
        ).await;

        let result = detector.on_price_update(
            price_to_bps(0.54), // 下跌 10%
            price_to_bps(0.40),
            3_000_000_000,
        ).await;

        assert_eq!(result, None, "10% 下跌不应触发 15% 阈值");
    }

    #[tokio::test]
    async fn test_window_expiration() {
        // 测试：窗口过期后不应检测到抛售
        let detector = DumpDetector::new(3_000_000_000, 0.15); // 3秒窗口

        // t=0: Up=0.60
        detector.on_price_update(
            price_to_bps(0.60),
            price_to_bps(0.40),
            0,
        ).await;

        // t=4秒: Up=0.50（但第一个快照已过期）
        let result = detector.on_price_update(
            price_to_bps(0.50),
            price_to_bps(0.40),
            4_000_000_000,
        ).await;

        assert_eq!(result, None, "窗口外的价格不应触发抛售");
    }

    #[tokio::test]
    async fn test_reset() {
        // 测试：重置功能
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        detector.on_price_update(
            price_to_bps(0.60),
            price_to_bps(0.40),
            1_000_000_000,
        ).await;

        detector.reset().await;

        let (up_size, down_size) = detector.get_window_size().await;
        assert_eq!(up_size, 0, "重置后 Up 历史应为空");
        assert_eq!(down_size, 0, "重置后 Down 历史应为空");
    }

    #[tokio::test]
    async fn test_multiple_snapshots_in_window() {
        // 测试：窗口内多个快照，找到最高点
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        // t=0: Up=0.50
        detector.on_price_update(
            price_to_bps(0.50),
            price_to_bps(0.50),
            0,
        ).await;

        // t=1s: Up=0.65（峰值）
        detector.on_price_update(
            price_to_bps(0.65),
            price_to_bps(0.50),
            1_000_000_000,
        ).await;

        // t=2s: Up=0.55（从 0.65 跌到 0.55 = 15.4%）
        let result = detector.on_price_update(
            price_to_bps(0.55),
            price_to_bps(0.50),
            2_000_000_000,
        ).await;

        assert_eq!(result, Some(DumpSide::Up), "应从窗口内最高点计算跌幅");
    }

    #[tokio::test]
    async fn test_edge_case_zero_price() {
        // 边界情况：价格为 0
        let detector = DumpDetector::new(3_000_000_000, 0.15);

        let result = detector.on_price_update(0, 0, 1_000_000_000).await;
        assert_eq!(result, None, "价格为 0 不应触发抛售");
    }
}
