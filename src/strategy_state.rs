use crate::dump_detector::DumpSide;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 策略状态枚举
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StrategyState {
    /// 等待抛售信号
    WaitingForDump,

    /// 第一阶段已执行，等待对冲条件
    WaitingForHedge,

    /// 正在执行第一阶段（买入单侧）
    ExecutingLeg1,

    /// 正在执行第二阶段（对冲）
    ExecutingLeg2,
}

/// 策略上下文 - 存储当前策略执行状态
#[derive(Debug, Clone)]
pub struct StrategyContext {
    /// 当前状态
    pub state: StrategyState,

    /// 第一阶段购买的侧（Up 或 Down）
    pub leg1_side: Option<DumpSide>,

    /// 第一阶段入场价格（BPS）
    pub leg1_entry_price: Option<u16>,

    /// 第一阶段购买的份额
    pub leg1_shares: Option<f64>,

    /// 第一阶段使用的 token_id
    pub leg1_token_id: Option<String>,

    /// 策略开始时间戳（纳秒）
    pub strategy_start_ns: Option<u64>,
}

impl Default for StrategyContext {
    fn default() -> Self {
        Self {
            state: StrategyState::WaitingForDump,
            leg1_side: None,
            leg1_entry_price: None,
            leg1_shares: None,
            leg1_token_id: None,
            strategy_start_ns: None,
        }
    }
}

/// 策略状态机
///
/// 负责管理两阶段交易策略的状态转换和条件检查
pub struct StrategyStateMachine {
    /// 共享状态
    context: Arc<RwLock<StrategyContext>>,

    /// 对冲条件：leg1_price + opposite_ask <= sum_target（例如 0.95）
    sum_target: f64,

    /// 早期退出：价格反弹达到此百分比时提前平仓（例如 0.10 = 10%）
    early_exit_profit_pct: Option<f64>,

    /// 硬止损：未对冲亏损达到此百分比时平仓（例如 0.50 = 50%）
    stop_loss_pct: Option<f64>,
}

impl StrategyStateMachine {
    /// 创建新的策略状态机
    ///
    /// # 参数
    /// - `sum_target`: 对冲条件阈值，例如 0.95
    /// - `early_exit_profit_pct`: 早期退出利润百分比（可选），例如 0.10
    /// - `stop_loss_pct`: 硬止损百分比（可选），例如 0.50
    pub fn new(
        sum_target: f64,
        early_exit_profit_pct: Option<f64>,
        stop_loss_pct: Option<f64>,
    ) -> Self {
        Self {
            context: Arc::new(RwLock::new(StrategyContext::default())),
            sum_target,
            early_exit_profit_pct,
            stop_loss_pct,
        }
    }

    /// 获取当前状态（只读）
    pub async fn get_state(&self) -> StrategyState {
        self.context.read().await.state
    }

    /// 获取完整上下文（只读）
    pub async fn get_context(&self) -> StrategyContext {
        self.context.read().await.clone()
    }

    /// 记录第一阶段执行
    ///
    /// 从 WaitingForDump → WaitingForHedge
    pub async fn record_leg1_execution(
        &self,
        side: DumpSide,
        entry_price: u16,
        shares: f64,
        token_id: String,
        timestamp_ns: u64,
    ) {
        let mut ctx = self.context.write().await;

        ctx.state = StrategyState::WaitingForHedge;
        ctx.leg1_side = Some(side);
        ctx.leg1_entry_price = Some(entry_price);
        ctx.leg1_shares = Some(shares);
        ctx.leg1_token_id = Some(token_id);
        ctx.strategy_start_ns = Some(timestamp_ns);
    }

    /// 检查对冲条件
    ///
    /// 公式：leg1_current_price + opposite_ask <= sum_target
    ///
    /// # 返回
    /// - `true` - 应该执行对冲
    /// - `false` - 条件未满足
    pub async fn check_hedge_condition(&self, up_ask: u16, down_ask: u16) -> bool {
        let ctx = self.context.read().await;

        if ctx.state != StrategyState::WaitingForHedge {
            return false;
        }

        let leg1_side = ctx.leg1_side.unwrap();

        // 计算当前价格和（BPS → f64）
        let sum = match leg1_side {
            DumpSide::Up => (up_ask as f64 + down_ask as f64) / 10000.0,
            DumpSide::Down => (down_ask as f64 + up_ask as f64) / 10000.0,
        };

        sum <= self.sum_target
    }

    /// 检查早期退出条件（价格反弹 >= early_exit_profit_pct）
    ///
    /// # 返回
    /// - `true` - 应该提前平仓
    /// - `false` - 条件未满足或未启用
    pub async fn check_early_exit(&self, current_price: u16) -> bool {
        let ctx = self.context.read().await;

        if ctx.state != StrategyState::WaitingForHedge {
            return false;
        }

        let Some(early_exit_pct) = self.early_exit_profit_pct else {
            return false; // 未启用早期退出
        };

        let leg1_entry = ctx.leg1_entry_price.unwrap() as f64;
        let current = current_price as f64;

        // 计算利润百分比
        let profit_pct = (current - leg1_entry) / leg1_entry;

        profit_pct >= early_exit_pct
    }

    /// 检查硬止损条件（亏损 >= stop_loss_pct）
    ///
    /// # 返回
    /// - `true` - 应该止损平仓
    /// - `false` - 条件未满足或未启用
    pub async fn check_stop_loss(&self, current_price: u16) -> bool {
        let ctx = self.context.read().await;

        if ctx.state != StrategyState::WaitingForHedge {
            return false;
        }

        let Some(stop_loss_pct) = self.stop_loss_pct else {
            return false; // 未启用止损
        };

        let leg1_entry = ctx.leg1_entry_price.unwrap() as f64;
        let current = current_price as f64;

        // 计算亏损百分比（注意：价格下跌时 unrealized_loss 为正）
        let unrealized_loss = (leg1_entry - current) / leg1_entry;

        unrealized_loss >= stop_loss_pct
    }

    /// 重置策略（用于轮次切换或策略完成）
    pub async fn reset(&self) {
        let mut ctx = self.context.write().await;
        *ctx = StrategyContext::default();
    }

    /// 标记策略完成（对冲成功或平仓完成）
    pub async fn mark_completed(&self) {
        self.reset().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price_to_bps(price: f64) -> u16 {
        (price * 10000.0) as u16
    }

    #[tokio::test]
    async fn test_initial_state() {
        let sm = StrategyStateMachine::new(0.95, Some(0.10), Some(0.50));
        assert_eq!(sm.get_state().await, StrategyState::WaitingForDump);
    }

    #[tokio::test]
    async fn test_record_leg1_execution() {
        let sm = StrategyStateMachine::new(0.95, Some(0.10), Some(0.50));

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        let ctx = sm.get_context().await;
        assert_eq!(ctx.state, StrategyState::WaitingForHedge);
        assert_eq!(ctx.leg1_side, Some(DumpSide::Up));
        assert_eq!(ctx.leg1_entry_price, Some(price_to_bps(0.50)));
        assert_eq!(ctx.leg1_shares, Some(20.0));
        assert_eq!(ctx.leg1_token_id, Some("token123".to_string()));
    }

    #[tokio::test]
    async fn test_hedge_condition_met() {
        let sm = StrategyStateMachine::new(0.95, None, None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // Up=0.50, Down=0.44 → sum=0.94 <= 0.95 ✅
        let should_hedge = sm
            .check_hedge_condition(price_to_bps(0.50), price_to_bps(0.44))
            .await;

        assert!(should_hedge, "对冲条件应满足（0.94 <= 0.95）");
    }

    #[tokio::test]
    async fn test_hedge_condition_not_met() {
        let sm = StrategyStateMachine::new(0.95, None, None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // Up=0.50, Down=0.46 → sum=0.96 > 0.95 ❌
        let should_hedge = sm
            .check_hedge_condition(price_to_bps(0.50), price_to_bps(0.46))
            .await;

        assert!(!should_hedge, "对冲条件不应满足（0.96 > 0.95）");
    }

    #[tokio::test]
    async fn test_early_exit_triggered() {
        let sm = StrategyStateMachine::new(0.95, Some(0.10), None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 价格从 0.50 涨到 0.56 = +12% > 10% ✅
        let should_exit = sm.check_early_exit(price_to_bps(0.56)).await;

        assert!(should_exit, "早期退出应触发（+12% > +10%）");
    }

    #[tokio::test]
    async fn test_early_exit_not_triggered() {
        let sm = StrategyStateMachine::new(0.95, Some(0.10), None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 价格从 0.50 涨到 0.54 = +8% < 10% ❌
        let should_exit = sm.check_early_exit(price_to_bps(0.54)).await;

        assert!(!should_exit, "早期退出不应触发（+8% < +10%）");
    }

    #[tokio::test]
    async fn test_stop_loss_triggered() {
        let sm = StrategyStateMachine::new(0.95, None, Some(0.50));

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.60),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 价格从 0.60 跌到 0.29 = -51.67% > 50% ✅
        let should_stop = sm.check_stop_loss(price_to_bps(0.29)).await;

        assert!(should_stop, "止损应触发（-51.67% > -50%）");
    }

    #[tokio::test]
    async fn test_stop_loss_not_triggered() {
        let sm = StrategyStateMachine::new(0.95, None, Some(0.50));

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.60),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 价格从 0.60 跌到 0.32 = -46.67% < 50% ❌
        let should_stop = sm.check_stop_loss(price_to_bps(0.32)).await;

        assert!(!should_stop, "止损不应触发（-46.67% < -50%）");
    }

    #[tokio::test]
    async fn test_reset() {
        let sm = StrategyStateMachine::new(0.95, Some(0.10), Some(0.50));

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        sm.reset().await;

        let ctx = sm.get_context().await;
        assert_eq!(ctx.state, StrategyState::WaitingForDump);
        assert_eq!(ctx.leg1_side, None);
        assert_eq!(ctx.leg1_entry_price, None);
    }

    #[tokio::test]
    async fn test_no_early_exit_when_disabled() {
        // early_exit_profit_pct = None（禁用）
        let sm = StrategyStateMachine::new(0.95, None, None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.50),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 即使价格大涨也不应触发
        let should_exit = sm.check_early_exit(price_to_bps(0.80)).await;

        assert!(!should_exit, "早期退出禁用时不应触发");
    }

    #[tokio::test]
    async fn test_no_stop_loss_when_disabled() {
        // stop_loss_pct = None（禁用）
        let sm = StrategyStateMachine::new(0.95, None, None);

        sm.record_leg1_execution(
            DumpSide::Up,
            price_to_bps(0.60),
            20.0,
            "token123".to_string(),
            1_000_000_000,
        )
        .await;

        // 即使价格暴跌也不应触发
        let should_stop = sm.check_stop_loss(price_to_bps(0.10)).await;

        assert!(!should_stop, "止损禁用时不应触发");
    }
}
