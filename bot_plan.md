# BTC 15分钟涨跌市场交易机器人技术方案

## 1. 项目概述

### 1.1 策略背景

本方案旨在基于现有的 Polymarket-Kalshi 套利机器人代码库，实现一个专注于 **Polymarket BTC 15分钟涨/跌市场**的自动化交易机器人。

**原始策略描述**（来自 bot_cn.md）：
- **市场类型**：BTC 15分钟涨/跌轮次市场
- **交易逻辑**：检测价格快速抛售，买入被抛售的一侧，然后在价格稳定后对冲
- **收益模式**：利用市场恐慌时的定价偏差，确保 `priceUP + priceDOWN < 1.00` 时建立双边仓位
- **回测数据**：保守参数集在4天内实现 86% ROI（$1,000 → $1,869）

### 1.2 当前代码库架构

现有代码库是一个**高性能的跨平台套利系统**，具备以下特性：

| 模块 | 功能 | 技术亮点 |
|------|------|----------|
| **类型系统** (types.rs) | 原子订单簿、全局状态管理 | 无锁设计、SIMD加速套利检测 |
| **WebSocket** (polymarket.rs) | 实时价格流订阅 | 自动重连、增量更新 |
| **订单执行** (polymarket_clob.rs) | CLOB订单提交 | EIP-712签名、FAK订单 |
| **执行引擎** (execution.rs) | 并发订单处理 | 去重、风险管理、自动平仓 |
| **断路器** (circuit_breaker.rs) | 风险保护 | 持仓限制、日损失限制 |
| **仓位跟踪** (position_tracker.rs) | P&L计算 | 异步批处理、持久化 |
| **市场发现** (discovery.rs) | 跨平台市场匹配 | 缓存机制、并行查询 |

---

## 2. 可复用组件分析

### 2.1 可直接复用的模块

| 模块 | 复用度 | 说明 |
|------|--------|------|
| **polymarket_clob.rs** | ✅ 100% | 订单执行逻辑完全兼容（FAK/IOC订单）|
| **polymarket.rs** | ✅ 90% | WebSocket价格流，需调整订阅市场逻辑 |
| **types.rs** | ✅ 80% | AtomicOrderbook可复用，需简化市场状态 |
| **circuit_breaker.rs** | ✅ 100% | 风险管理机制通用 |
| **position_tracker.rs** | ✅ 90% | 仓位跟踪需调整为单市场模式 |
| **cache.rs** | ❌ 0% | 团队代码映射不适用于BTC市场 |
| **discovery.rs** | ❌ 10% | 跨平台匹配逻辑不适用，但缓存框架可参考 |
| **kalshi.rs** | ❌ 0% | 不需要Kalshi集成 |
| **execution.rs** | 🔄 60% | 需重构为单平台顺序执行 |
| **config.rs** | 🔄 30% | 需替换为BTC市场配置 |

### 2.2 需要新增的组件

1. **轮次管理器** (`round_manager.rs`)
   - 自动检测并切换到当前活跃的15分钟轮次
   - 跟踪轮次开始时间和剩余秒数
   - 轮次切换时清理未完成的交易

2. **抛售检测器** (`dump_detector.rs`)
   - 实时监控价格变动速率
   - 检测快速抛售信号（movePct阈值）
   - 时间窗口控制（windowMin）

3. **策略状态机** (`strategy_state.rs`)
   - 第一阶段：等待抛售信号
   - 第二阶段：等待对冲条件
   - 循环完成/轮次切换处理

4. **回测引擎** (`backtest.rs`)
   - 加载历史快照数据
   - 确定性重放策略逻辑
   - ROI计算和参数优化

5. **数据记录器** (`data_logger.rs`)
   - 实时记录订单簿快照到磁盘
   - 支持回测数据集构建

---

## 3. 架构设计

### 3.1 系统架构图

```
┌─────────────────────────────────────────────────────────────────────────┐
│                      BTC 15分钟涨跌交易机器人                             │
└─────────────────────────────────────────────────────────────────────────┘
                                   │
                 ┌─────────────────┼─────────────────┐
                 │                 │                 │
                 ▼                 ▼                 ▼
        ┌──────────────┐  ┌──────────────┐  ┌──────────────┐
        │ 轮次管理器    │  │ WebSocket    │  │ 策略状态机    │
        │ (自动切换)    │  │ (价格流)     │  │ (两阶段循环)  │
        └──────┬───────┘  └──────┬───────┘  └──────┬───────┘
               │                 │                 │
               └────────┬────────┴────────┬────────┘
                        │                 │
                        ▼                 ▼
               ┌──────────────┐  ┌──────────────┐
               │ 抛售检测器    │  │ 原子订单簿    │
               │ (价格监控)    │  │ (无锁更新)    │
               └──────┬───────┘  └──────┬───────┘
                      │                 │
                      └────────┬────────┘
                               │
                               ▼
                      ┌──────────────┐
                      │ 执行引擎      │
                      │ (顺序下单)    │
                      └──────┬───────┘
                             │
                ┌────────────┼────────────┐
                │            │            │
                ▼            ▼            ▼
        ┌──────────┐ ┌──────────┐ ┌──────────┐
        │ 仓位跟踪 │ │ 断路器   │ │ 数据记录 │
        └──────────┘ └──────────┘ └──────────┘
```

### 3.2 核心流程图

```
┌──────────────────────────────────────────────────────────────────┐
│                         启动流程                                  │
└──────────────────────────────────────────────────────────────────┘

1. 加载配置参数 (shares, sum, movePct, windowMin)
                 │
                 ▼
2. 初始化 Polymarket 客户端 (CLOB + WebSocket)
                 │
                 ▼
3. 发现并订阅当前活跃的 BTC 15分钟轮次市场
                 │
                 ▼
4. 启动 WebSocket 价格流 (UP token + DOWN token)
                 │
                 ▼
5. 进入策略主循环

┌──────────────────────────────────────────────────────────────────┐
│                         策略主循环                                │
└──────────────────────────────────────────────────────────────────┘

State: WAITING_FOR_DUMP
  │
  ├─► 检查轮次是否切换
  │    ├─ 是 → 重置状态，订阅新轮次
  │    └─ 否 → 继续
  │
  ├─► 检查时间窗口 (是否在轮次开始后 windowMin 分钟内)
  │    ├─ 否 → 等待下一轮次
  │    └─ 是 → 继续
  │
  ├─► 监控价格变动
  │    └─► 检测快速抛售 (约3秒内下跌 >= movePct)
  │         ├─ UP侧抛售 → 触发第一阶段：买入 UP
  │         └─ DOWN侧抛售 → 触发第一阶段：买入 DOWN
  │
  └─► 第一阶段执行
       │
       ▼
State: WAITING_FOR_HEDGE
  │
  ├─► 记录 leg1_entry_price
  │
  ├─► 永不再买入同侧
  │
  ├─► 监控对冲条件: leg1_entry_price + opposite_ask <= sumTarget
  │    │
  │    ├─ 满足 → 触发第二阶段：买入对侧
  │    │         └─► 完成循环，返回 WAITING_FOR_DUMP
  │    │
  │    └─ 不满足 → 继续等待
  │         │
  │         └─► 检查轮次是否即将结束
  │              ├─ 是 → 放弃对冲，记录损失（保守估计）
  │              └─ 否 → 继续等待
```

### 3.3 数据流设计

```
WebSocket (Polymarket)
    │
    ├─► BookSnapshot / PriceChangeEvent
    │
    ▼
AtomicOrderbook
    │ (无锁更新)
    │
    ├─► up_ask, up_size
    └─► down_ask, down_size
         │
         ▼
DumpDetector (价格监控)
    │
    ├─► 计算3秒滑动窗口内的价格变化率
    │
    └─► 如果 |price_change| >= movePct
         │
         ▼
StrategyStateMachine
    │
    ├─► State: WAITING_FOR_DUMP
    │    └─► 触发第一阶段
    │
    └─► State: WAITING_FOR_HEDGE
         │
         └─► 检查: leg1_price + opposite_ask <= sumTarget
              │
              └─► 触发第二阶段
                   │
                   ▼
ExecutionEngine
    │
    ├─► Leg1: buy_fak(dumped_side, shares)
    │    └─► 等待成交确认
    │
    └─► Leg2: buy_fak(opposite_side, shares)
         │
         ▼
PositionTracker
    │
    ├─► 记录成交价格、数量、费用
    │
    └─► 计算保证利润
         = (shares × 1.00) - (leg1_cost + leg2_cost)
```

---

## 4. 详细实现方案

### 4.1 轮次管理器 (round_manager.rs)

#### 4.1.1 数据结构

```rust
pub struct RoundInfo {
    pub round_slug: String,          // "eth-updown-15m-1767276000"
    pub up_token_id: String,         // CLOB token ID for UP
    pub down_token_id: String,       // CLOB token ID for DOWN
    pub start_timestamp: u64,        // Unix时间戳（秒）
    pub end_timestamp: u64,          // Unix时间戳（秒）
    pub condition_id: String,        // Polymarket conditionId
    pub market_id: String,           // Polymarket市场ID
    pub asset: String,               // "BTC" | "ETH" | "SOL"
}

pub struct RoundManager {
    current_rounds: Arc<RwLock<HashMap<String, RoundInfo>>>,  // asset -> RoundInfo
    poly_client: Arc<PolymarketAsyncClient>,
    enabled_assets: Vec<String>,     // 配置的资产列表
}
```

#### 4.1.2 Slug 生成规则

根据 data.json 的格式，slug 格式为：`{asset}-updown-15m-{timestamp}`

```rust
impl RoundManager {
    // 根据当前时间生成下一轮次的slug
    pub fn generate_round_slug(asset: &str, start_time: u64) -> String {
        // 格式: eth-updown-15m-1767276000
        format!("{}-updown-15m-{}", asset.to_lowercase(), start_time)
    }

    // 计算下一个15分钟轮次的开始时间
    pub fn calculate_next_round_time(now: u64) -> u64 {
        let fifteen_min = 900; // 15 * 60
        ((now / fifteen_min) + 1) * fifteen_min
    }

    // 示例：
    // 当前时间: 2026-01-01 14:07:30 (1735740450)
    // 下一轮次: 2026-01-01 14:15:00 (1735740900)
    // slug: "btc-updown-15m-1735740900"
}
```

#### 4.1.3 市场数据结构（参考 data.json）

```rust
#[derive(Deserialize)]
pub struct PolymarketResponse {
    pub id: String,
    pub question: String,
    pub slug: String,
    pub condition_id: String,
    #[serde(rename = "clobTokenIds")]
    pub clob_token_ids: String,  // JSON array string
    #[serde(rename = "endDate")]
    pub end_date: String,        // ISO 8601
    #[serde(rename = "eventStartTime")]
    pub event_start_time: String, // ISO 8601
    pub active: bool,
    pub closed: bool,
}

impl PolymarketResponse {
    pub fn parse_token_ids(&self) -> Result<(String, String)> {
        // clobTokenIds: "[\"108702...\", \"14771...\"]"
        let ids: Vec<String> = serde_json::from_str(&self.clob_token_ids)?;
        if ids.len() != 2 {
            return Err(anyhow!("Expected 2 token IDs"));
        }
        Ok((ids[0].clone(), ids[1].clone()))  // (UP, DOWN)
    }
}
```

#### 4.1.4 多资产支持

```rust
pub struct RoundManagerConfig {
    pub enabled_assets: Vec<String>,  // ["BTC", "ETH", "SOL"]
    pub check_interval_secs: u64,     // 检查间隔（默认30秒）
}

impl RoundManager {
    pub fn new(config: RoundManagerConfig, poly_client: Arc<PolymarketAsyncClient>) -> Self {
        Self {
            current_rounds: Arc::new(RwLock::new(HashMap::new())),
            poly_client,
            enabled_assets: config.enabled_assets,
        }
    }

    // 获取指定资产的当前活跃轮次
    pub async fn fetch_active_round(&self, asset: &str) -> Result<RoundInfo> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let next_round_time = Self::calculate_next_round_time(now);
        let slug = Self::generate_round_slug(asset, next_round_time);

        // 调用 Polymarket Gamma API
        // GET https://gamma-api.polymarket.com/markets?slug={slug}
        let url = format!("https://gamma-api.polymarket.com/markets?slug={}", slug);
        let response: Vec<PolymarketResponse> = self.poly_client
            .http_client()
            .get(&url)
            .send()
            .await?
            .json()
            .await?;

        let market = response.first()
            .ok_or_else(|| anyhow!("Market not found for slug: {}", slug))?;

        if !market.active || market.closed {
            return Err(anyhow!("Market {} is not active", slug));
        }

        let (up_token_id, down_token_id) = market.parse_token_ids()?;
        let start_timestamp = chrono::DateTime::parse_from_rfc3339(&market.event_start_time)?
            .timestamp() as u64;
        let end_timestamp = chrono::DateTime::parse_from_rfc3339(&market.end_date)?
            .timestamp() as u64;

        Ok(RoundInfo {
            round_slug: market.slug.clone(),
            up_token_id,
            down_token_id,
            start_timestamp,
            end_timestamp,
            condition_id: market.condition_id.clone(),
            market_id: market.id.clone(),
            asset: asset.to_uppercase(),
        })
    }

    // 获取所有启用资产的活跃轮次
    pub async fn fetch_all_active_rounds(&self) -> Result<HashMap<String, RoundInfo>> {
        let mut rounds = HashMap::new();

        for asset in &self.enabled_assets {
            match self.fetch_active_round(asset).await {
                Ok(round) => {
                    info!("✅ 获取{}轮次成功: {}", asset, round.round_slug);
                    rounds.insert(asset.clone(), round);
                }
                Err(e) => {
                    warn!("⚠️ 获取{}轮次失败: {}", asset, e);
                }
            }
        }

        Ok(rounds)
    }

    // 检查轮次是否已结束（不提前切换）
    pub fn is_round_ended(&self, asset: &str) -> bool {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let rounds = self.current_rounds.blocking_read();

        if let Some(round) = rounds.get(asset) {
            now >= round.end_timestamp
        } else {
            false
        }
    }

    // 获取下一轮次（仅在当前轮次结束后调用）
    pub async fn fetch_next_round(&self, asset: &str) -> Result<RoundInfo> {
        info!("🔄 {}当前轮次已结束，获取下一轮", asset);
        self.fetch_active_round(asset).await
    }

    // 获取当前轮次剩余秒数
    pub fn get_remaining_seconds(&self, asset: &str) -> Option<u64> {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let rounds = self.current_rounds.blocking_read();

        rounds.get(asset).map(|round| {
            if now < round.end_timestamp {
                round.end_timestamp - now
            } else {
                0
            }
        })
    }
}
```

#### 4.1.5 配置示例

```bash
# 环境变量配置
ENABLED_ASSETS=BTC,ETH,SOL        # 监听的资产列表
ROUND_CHECK_INTERVAL=30           # 轮次检查间隔（秒）

# 或使用配置文件 round_config.toml
[round_manager]
enabled_assets = ["BTC", "ETH", "SOL"]
check_interval_secs = 30
pre_rotation_secs = 60  # 提前60秒切换轮次
```

#### 4.1.6 使用示例

```rust
// 初始化轮次管理器
let config = RoundManagerConfig {
    enabled_assets: vec!["BTC".to_string(), "ETH".to_string(), "SOL".to_string()],
    check_interval_secs: 30,
};
let round_manager = Arc::new(RoundManager::new(config, poly_async.clone()));

// 获取所有活跃轮次
let active_rounds = round_manager.fetch_all_active_rounds().await?;

for (asset, round) in active_rounds {
    info!("资产: {}", asset);
    info!("  轮次: {}", round.round_slug);
    info!("  UP token: {}", round.up_token_id);
    info!("  DOWN token: {}", round.down_token_id);
    info!("  开始时间: {}", round.start_timestamp);
    info!("  结束时间: {}", round.end_timestamp);
}

// 监控轮次切换（在主循环中处理，不是独立任务）
// 注意：轮次切换由主策略循环控制，避免与平仓策略冲突
```

### 4.2 抛售检测器 (dump_detector.rs)

#### 4.2.1 数据结构

```rust
pub struct PriceSnapshot {
    pub timestamp_ns: u64,
    pub up_price: u16,    // 单位：美分
    pub down_price: u16,
}

pub struct DumpDetector {
    // 使用固定大小环形缓冲区存储最近N个快照
    price_history_up: Arc<Mutex<VecDeque<PriceSnapshot>>>,
    price_history_down: Arc<Mutex<VecDeque<PriceSnapshot>>>,

    detection_window_ns: u64,  // 检测窗口（默认3秒 = 3_000_000_000纳秒）
    move_threshold: f64,       // 抛售阈值（如0.15 = 15%）
}
```

#### 4.2.2 核心算法

```rust
impl DumpDetector {
    // 检测UP侧是否发生快速抛售
    pub fn check_dump_up(&self, current_price: u16, now_ns: u64) -> bool {
        let mut history = self.price_history_up.lock().unwrap();

        // 添加当前快照
        history.push_back(PriceSnapshot {
            timestamp_ns: now_ns,
            up_price: current_price,
            down_price: 0,  // 仅需要当前侧的价格
        });

        // 清理超出窗口的历史数据
        while let Some(oldest) = history.front() {
            if now_ns - oldest.timestamp_ns > self.detection_window_ns {
                history.pop_front();
            } else {
                break;
            }
        }

        // 计算窗口内的最大价格和当前价格的差异
        if let Some(max_snapshot) = history.iter().max_by_key(|s| s.up_price) {
            let price_drop = (max_snapshot.up_price - current_price) as f64 / max_snapshot.up_price as f64;

            if price_drop >= self.move_threshold {
                info!("🚨 UP侧抛售检测: {:.2}% 下跌 (从{}¢降至{}¢)",
                      price_drop * 100.0, max_snapshot.up_price, current_price);
                return true;
            }
        }

        false
    }

    // DOWN侧同理
    pub fn check_dump_down(&self, current_price: u16, now_ns: u64) -> bool {
        // 对称实现
    }
}
```

### 4.3 策略状态机 (strategy_state.rs)

#### 4.3.1 状态定义

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StrategyState {
    WaitingForDump,      // 等待抛售信号
    WaitingForHedge,     // 等待对冲条件
    ExecutingLeg1,       // 执行第一阶段
    ExecutingLeg2,       // 执行第二阶段
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DumpedSide {
    Up,
    Down,
}

pub struct StrategyContext {
    pub state: StrategyState,
    pub dumped_side: Option<DumpedSide>,
    pub leg1_entry_price: Option<u16>,  // 美分
    pub leg1_fill_size: Option<u64>,    // 实际成交份额
    pub round_slug: String,
    pub window_start_ns: u64,           // 窗口开始时间
    pub window_end_ns: u64,             // 窗口结束时间
}
```

#### 4.3.2 状态转换逻辑

```rust
pub struct StrategyStateMachine {
    context: Arc<Mutex<StrategyContext>>,
    config: StrategyConfig,
}

pub struct StrategyConfig {
    pub shares: u64,                // 每条腿的份额数
    pub sum_target: f64,            // 对冲阈值（如0.95）
    pub move_pct: f64,              // 抛售阈值（如0.15）
    pub window_min: u64,            // 时间窗口（分钟）
    pub early_exit_profit_pct: f64, // 提前平仓利润阈值（默认0.10 = 10%）
    pub stop_loss_pct: f64,         // 硬止损阈值（默认0.50 = 50%）✅
    pub stop_loss_enabled: bool,    // 是否启用硬止损（默认true）✅
}

impl StrategyStateMachine {
    // 处理抛售信号
    pub async fn on_dump_detected(&self, side: DumpedSide, price: u16) -> Result<()> {
        let mut ctx = self.context.lock().unwrap();

        if ctx.state != StrategyState::WaitingForDump {
            return Ok(());  // 忽略非等待状态的抛售
        }

        // 检查时间窗口
        let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;
        if now_ns > ctx.window_end_ns {
            warn!("抛售发生在时间窗口外，忽略");
            return Ok(());
        }

        info!("✅ 触发第一阶段: {:?}侧抛售至{}¢", side, price);

        ctx.state = StrategyState::ExecutingLeg1;
        ctx.dumped_side = Some(side);
        ctx.leg1_entry_price = Some(price);

        Ok(())
    }

    // 检查对冲条件
    pub fn check_hedge_condition(&self, up_ask: u16, down_ask: u16) -> bool {
        let ctx = self.context.lock().unwrap();

        if ctx.state != StrategyState::WaitingForHedge {
            return false;
        }

        let leg1_price = ctx.leg1_entry_price.unwrap();
        let opposite_ask = match ctx.dumped_side.unwrap() {
            DumpedSide::Up => down_ask,
            DumpedSide::Down => up_ask,
        };

        let total_cost = leg1_price + opposite_ask;
        let sum_target_cents = (self.config.sum_target * 100.0) as u16;

        if total_cost <= sum_target_cents {
            info!("✅ 对冲条件满足: {}¢ + {}¢ = {}¢ <= {}¢",
                  leg1_price, opposite_ask, total_cost, sum_target_cents);
            return true;
        }

        false
    }

    // 检查提前平仓条件（价格反弹）
    pub fn check_early_exit(&self, current_price: u16) -> bool {
        let ctx = self.context.lock().unwrap();

        if ctx.state != StrategyState::WaitingForHedge {
            return false;
        }

        let leg1_price = ctx.leg1_entry_price.unwrap();
        let profit_pct = (current_price as f64 - leg1_price as f64) / leg1_price as f64;

        // 如果价格反弹超过阈值（如10%），提前平仓获利
        if profit_pct >= 0.10 {
            info!("💰 提前平仓条件满足: 价格从{}¢反弹至{}¢ (+{:.2}%)",
                  leg1_price, current_price, profit_pct * 100.0);
            return true;
        }

        false
    }

    // 完成循环
    pub fn complete_cycle(&self) {
        let mut ctx = self.context.lock().unwrap();
        info!("🎯 循环完成，返回等待状态");
        ctx.state = StrategyState::WaitingForDump;
        ctx.dumped_side = None;
        ctx.leg1_entry_price = None;
        ctx.leg1_fill_size = None;
    }

    // 轮次切换时重置
    pub fn reset_for_new_round(&self, round_slug: String, start_ns: u64, window_min: u64) {
        let mut ctx = self.context.lock().unwrap();
        ctx.state = StrategyState::WaitingForDump;
        ctx.dumped_side = None;
        ctx.leg1_entry_price = None;
        ctx.leg1_fill_size = None;
        ctx.round_slug = round_slug;
        ctx.window_start_ns = start_ns;
        ctx.window_end_ns = start_ns + (window_min * 60 * 1_000_000_000);
    }
}
```

### 4.4 执行引擎调整 (execution.rs)

#### 4.4.1 新增单平台顺序执行

```rust
pub struct BtcExecutionEngine {
    poly_async: Arc<PolymarketAsyncClient>,
    circuit_breaker: Arc<CircuitBreaker>,
    position_channel: mpsc::Sender<FillRecord>,
    dry_run: bool,
}

impl BtcExecutionEngine {
    // 卖出仓位（提前平仓或止损）
    pub async fn sell_position(
        &self,
        side: DumpedSide,
        token_id: &str,
        shares: u64,
        expected_price: u16,
    ) -> Result<LegFillResult> {

        if self.dry_run {
            info!("[DRY RUN] 卖出 {:?} {} shares @ {}¢", side, shares, expected_price);
            return Ok(LegFillResult {
                filled_size: shares,
                avg_price: expected_price,
                order_id: "dry_run_sell".to_string(),
            });
        }

        info!("📤 卖出: {:?} {} shares @ ~{}¢", side, shares, expected_price);

        // 注意：sell_fak 参数是 (token_id, price, size)
        // price 单位是 f64 美元（0.0 - 1.0），不是 BPS
        let price = expected_price as f64 / 100.0;  // 美分转美元
        let size = shares as f64;
        let result = self.poly_async.sell_fak(token_id, price, size).await?;

        info!("✅ 卖出成交: {} shares @ {:.2}¢", result.filled_size, result.avg_price);

        self.position_channel.send(FillRecord {
            market_id: "btc-15min".to_string(),
            side: format!("{:?}_SELL", side),
            size: result.filled_size,
            price: result.avg_price,
            timestamp: SystemTime::now(),
        }).await?;

        Ok(result)
    }

    // 执行第一阶段（买入被抛售的一侧）
    pub async fn execute_leg1(
        &self,
        side: DumpedSide,
        token_id: &str,
        shares: u64,
        expected_price: u16,
    ) -> Result<LegFillResult> {

        // 1. 断路器检查
        self.circuit_breaker.can_execute("btc-15min", shares as i64).await?;

        // 2. 干运行检查
        if self.dry_run {
            info!("[DRY RUN] Leg1 买入 {:?} {} shares @ {}¢", side, shares, expected_price);
            return Ok(LegFillResult {
                filled_size: shares,
                avg_price: expected_price,
                order_id: "dry_run_leg1".to_string(),
            });
        }

        // 3. 下单（FAK = Fill-And-Kill = IOC）
        info!("📤 Leg1: 买入 {:?} {} shares @ ~{}¢", side, shares, expected_price);

        // 注意：buy_fak 参数是 (token_id, price, size)
        // price 单位是 f64 美元（0.0 - 1.0），size 单位是份额数
        let price = expected_price as f64 / 100.0;  // 美分转美元
        let size = shares as f64;
        let result = self.poly_async.buy_fak(token_id, price, size).await?;

        info!("✅ Leg1 成交: {} shares @ {:.2}¢", result.filled_size, result.avg_price);

        // 4. 记录到仓位跟踪器
        self.position_channel.send(FillRecord {
            market_id: "btc-15min".to_string(),
            side: format!("{:?}", side),
            size: result.filled_size,
            price: result.avg_price,
            timestamp: SystemTime::now(),
        }).await?;

        Ok(result)
    }

    // 执行第二阶段（对冲）
    pub async fn execute_leg2(
        &self,
        side: DumpedSide,  // 对冲侧（与leg1相反）
        token_id: &str,
        shares: u64,
        expected_price: u16,
    ) -> Result<LegFillResult> {
        // 类似leg1实现
    }
}

pub struct LegFillResult {
    pub filled_size: u64,
    pub avg_price: u16,
    pub order_id: String,
}
```

### 4.5 主程序集成 (main.rs)

#### 4.5.1 初始化流程

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // 1. 加载配置
    let config = StrategyConfig {
        shares: env::var("STRATEGY_SHARES")?.parse()?,
        sum_target: env::var("STRATEGY_SUM_TARGET")?.parse().unwrap_or(0.95),
        move_pct: env::var("STRATEGY_MOVE_PCT")?.parse().unwrap_or(0.15),
        window_min: env::var("STRATEGY_WINDOW_MIN")?.parse().unwrap_or(2),
    };

    info!("策略参数: shares={}, sum={}, move={}, window={}分钟",
          config.shares, config.sum_target, config.move_pct, config.window_min);

    // 2. 初始化Polymarket客户端
    let poly_private_key = env::var("POLY_PRIVATE_KEY")?;
    let poly_funder = env::var("POLY_FUNDER")?;
    let poly_async = Arc::new(PolymarketAsyncClient::new(
        "https://clob.polymarket.com",  // CLOB API host
        137,                             // Polygon主网chain_id
        &poly_private_key,
        &poly_funder,
    )?);

    // 3. 初始化轮次管理器
    let round_manager = Arc::new(RoundManager::new(poly_async.clone()));
    let current_round = round_manager.fetch_active_round().await?;
    info!("📍 当前轮次: {}", current_round.round_slug);

    // 4. 初始化抛售检测器
    let dump_detector = Arc::new(DumpDetector::new(
        3_000_000_000,  // 3秒窗口
        config.move_pct,
    ));

    // 5. 初始化策略状态机
    let state_machine = Arc::new(StrategyStateMachine::new(config.clone()));
    state_machine.reset_for_new_round(
        current_round.round_slug.clone(),
        current_round.start_timestamp * 1_000_000_000,  // 转纳秒
        config.window_min,
    );

    // 6. 初始化原子订单簿
    let orderbook = Arc::new(AtomicOrderbook::new());

    // 7. 初始化执行引擎、断路器、仓位跟踪
    let (position_tx, position_rx) = mpsc::channel(100);
    let circuit_breaker = Arc::new(CircuitBreaker::new(CircuitBreakerConfig::from_env()));
    let exec_engine = Arc::new(BtcExecutionEngine::new(
        poly_async.clone(),
        circuit_breaker.clone(),
        position_tx.clone(),
        env::var("DRY_RUN").unwrap_or_default() == "1",
    ));

    let position_tracker = Arc::new(PositionTracker::new(position_rx));
    tokio::spawn(async move {
        position_tracker.run().await;
    });

    // 8. 启动WebSocket
    let ws_handle = tokio::spawn(run_btc_websocket(
        current_round.up_token_id.clone(),
        current_round.down_token_id.clone(),
        orderbook.clone(),
        dump_detector.clone(),
        state_machine.clone(),
        exec_engine.clone(),
    ));

    // 9. 启动轮次监控（在主循环中内联处理，不独立spawn）
    // 注意：轮次切换由主循环的优先级0检查处理，避免与平仓策略冲突

    // 10. 主循环
    tokio::select! {
        _ = ws_handle => warn!("WebSocket任务退出"),
        _ = rotation_handle => warn!("轮次监控退出"),
        _ = tokio::signal::ctrl_c() => {
            info!("收到终止信号");
        }
    }

    Ok(())
}
```

#### 4.5.2 WebSocket价格流处理

```rust
async fn run_btc_websocket(
    up_token_id: String,
    down_token_id: String,
    orderbook: Arc<AtomicOrderbook>,
    dump_detector: Arc<DumpDetector>,
    state_machine: Arc<StrategyStateMachine>,
    exec_engine: Arc<BtcExecutionEngine>,
) -> Result<()> {

    // 订阅UP和DOWN两个token的价格流
    let market_ids = vec![up_token_id.clone(), down_token_id.clone()];

    // 复用现有的polymarket::run_ws函数
    // 但需要自定义价格更新回调

    loop {
        match recv_price_update().await {
            Ok((token_id, new_ask, new_size)) => {
                let now_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos() as u64;

                // 更新订单簿
                if token_id == up_token_id {
                    orderbook.update_yes(new_ask, new_size);

                    // 检测UP侧抛售
                    if dump_detector.check_dump_up(new_ask, now_ns) {
                        state_machine.on_dump_detected(DumpedSide::Up, new_ask).await?;

                        // 触发第一阶段执行
                        if let Err(e) = exec_engine.execute_leg1(
                            DumpedSide::Up,
                            &up_token_id,
                            config.shares,
                            new_ask,
                        ).await {
                            error!("Leg1执行失败: {}", e);
                        } else {
                            state_machine.transition_to_waiting_hedge();
                        }
                    }
                } else if token_id == down_token_id {
                    orderbook.update_no(new_ask, new_size);

                    // DOWN侧同理
                }

                // 检查提前平仓条件（价格反弹）
                // 注意：需要先获取订单簿价格
                let (up_ask, down_ask) = orderbook.get_both_asks();
                let current_side_price = if token_id == up_token_id { up_ask } else { down_ask };
                if state_machine.check_early_exit(current_side_price) {
                    let leg1_side = state_machine.get_leg1_side();
                    let token = if leg1_side == DumpedSide::Up { &up_token_id } else { &down_token_id };

                    // 卖出第一阶段仓位
                    if let Err(e) = exec_engine.sell_position(
                        leg1_side,
                        token,
                        config.shares,
                        current_side_price,
                    ).await {
                        error!("提前平仓失败: {}", e);
                    } else {
                        info!("✅ 提前平仓成功");
                        state_machine.complete_cycle();
                    }
                }

                // 检查对冲条件（仅在等待对冲状态）
                if !state_machine.check_early_exit(current_side_price)
                    && state_machine.check_hedge_condition(up_ask, down_ask) {
                    let opposite_side = state_machine.get_opposite_side();
                    let opposite_token = if opposite_side == DumpedSide::Up {
                        &up_token_id
                    } else {
                        &down_token_id
                    };

                    // 触发第二阶段执行
                    if let Err(e) = exec_engine.execute_leg2(
                        opposite_side,
                        opposite_token,
                        config.shares,
                        if opposite_side == DumpedSide::Up { up_ask } else { down_ask },
                    ).await {
                        error!("Leg2执行失败: {}", e);
                    } else {
                        state_machine.complete_cycle();
                    }
                }
            }
            Err(e) => {
                error!("WebSocket错误: {}", e);
                // 重连逻辑
            }
        }
    }
}
```

### 4.6 数据记录器 (data_logger.rs)

#### 4.6.1 快照格式

```rust
#[derive(Serialize, Deserialize)]
pub struct OrderbookSnapshot {
    pub timestamp_ns: u64,
    pub round_slug: String,
    pub remaining_secs: u64,
    pub up_token_id: String,
    pub down_token_id: String,
    pub up_ask: u16,      // 美分
    pub down_ask: u16,
    pub up_size: u16,     // 可用份额（美分单位）
    pub down_size: u16,
}
```

#### 4.6.2 写入逻辑

```rust
pub struct DataLogger {
    writer: Arc<Mutex<BufWriter<File>>>,
}

impl DataLogger {
    pub fn new(path: &str) -> Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;

        Ok(Self {
            writer: Arc::new(Mutex::new(BufWriter::new(file))),
        })
    }

    pub async fn log_snapshot(&self, snapshot: OrderbookSnapshot) -> Result<()> {
        let json = serde_json::to_string(&snapshot)?;
        let mut writer = self.writer.lock().unwrap();
        writeln!(writer, "{}", json)?;
        Ok(())
    }

    pub async fn flush(&self) -> Result<()> {
        let mut writer = self.writer.lock().unwrap();
        writer.flush()?;
        Ok(())
    }
}
```

### 4.7 回测引擎 (backtest.rs)

#### 4.7.1 核心结构

```rust
pub struct BacktestEngine {
    snapshots: Vec<OrderbookSnapshot>,
    config: StrategyConfig,
    initial_balance: f64,
}

pub struct BacktestResult {
    pub total_cycles: usize,
    pub successful_cycles: usize,
    pub failed_cycles: usize,
    pub total_profit: f64,
    pub total_loss: f64,
    pub final_balance: f64,
    pub roi: f64,
    pub trades: Vec<TradeRecord>,
}

pub struct TradeRecord {
    pub round_slug: String,
    pub leg1_side: DumpedSide,
    pub leg1_price: u16,
    pub leg2_price: Option<u16>,
    pub shares: u64,
    pub profit: f64,
    pub completed: bool,
}
```

#### 4.7.2 回测逻辑

```rust
impl BacktestEngine {
    pub fn load_from_file(path: &str, config: StrategyConfig, initial_balance: f64) -> Result<Self> {
        let file = File::open(path)?;
        let reader = BufReader::new(file);

        let mut snapshots = Vec::new();
        for line in reader.lines() {
            let snapshot: OrderbookSnapshot = serde_json::from_str(&line?)?;
            snapshots.push(snapshot);
        }

        snapshots.sort_by_key(|s| s.timestamp_ns);

        Ok(Self { snapshots, config, initial_balance })
    }

    pub fn run(&self) -> BacktestResult {
        let mut state = StrategyState::WaitingForDump;
        let mut current_round = String::new();
        let mut balance = self.initial_balance;
        let mut trades = Vec::new();

        let mut leg1_price: Option<u16> = None;
        let mut leg1_side: Option<DumpedSide> = None;
        let mut dump_detector = DumpDetector::new(3_000_000_000, self.config.move_pct);

        for snapshot in &self.snapshots {
            // 轮次切换检测
            if snapshot.round_slug != current_round {
                if state == StrategyState::WaitingForHedge {
                    // 未完成对冲，记录为损失
                    let loss = (self.config.shares as f64) * (leg1_price.unwrap() as f64 / 100.0);
                    balance -= loss;

                    trades.push(TradeRecord {
                        round_slug: current_round.clone(),
                        leg1_side: leg1_side.unwrap(),
                        leg1_price: leg1_price.unwrap(),
                        leg2_price: None,
                        shares: self.config.shares,
                        profit: -loss,
                        completed: false,
                    });
                }

                // 重置状态和时间窗口
                state = StrategyState::WaitingForDump;
                leg1_price = None;
                leg1_side = None;
                current_round = snapshot.round_slug.clone();
                dump_detector = DumpDetector::new(3_000_000_000, self.config.move_pct);

                // 重置窗口开始时间（重要！）
                info!("🔄 回测：轮次切换到 {}, 窗口重置", current_round);
            }

            match state {
                StrategyState::WaitingForDump => {
                    // 检测抛售
                    if dump_detector.check_dump_up(snapshot.up_ask, snapshot.timestamp_ns) {
                        state = StrategyState::WaitingForHedge;
                        leg1_side = Some(DumpedSide::Up);
                        leg1_price = Some(snapshot.up_ask);
                    } else if dump_detector.check_dump_down(snapshot.down_ask, snapshot.timestamp_ns) {
                        state = StrategyState::WaitingForHedge;
                        leg1_side = Some(DumpedSide::Down);
                        leg1_price = Some(snapshot.down_ask);
                    }
                }

                StrategyState::WaitingForHedge => {
                    let opposite_ask = match leg1_side.unwrap() {
                        DumpedSide::Up => snapshot.down_ask,
                        DumpedSide::Down => snapshot.up_ask,
                    };

                    let total_cost = leg1_price.unwrap() + opposite_ask;
                    let sum_target_cents = (self.config.sum_target * 100.0) as u16;

                    if total_cost <= sum_target_cents {
                        // 对冲成功
                        let cost = (total_cost as f64 / 100.0) * self.config.shares as f64;
                        let revenue = self.config.shares as f64;  // 保证收益$1.00/份
                        let profit = revenue - cost;

                        balance += profit;

                        trades.push(TradeRecord {
                            round_slug: current_round.clone(),
                            leg1_side: leg1_side.unwrap(),
                            leg1_price: leg1_price.unwrap(),
                            leg2_price: Some(opposite_ask),
                            shares: self.config.shares,
                            profit,
                            completed: true,
                        });

                        // 返回等待状态
                        state = StrategyState::WaitingForDump;
                        leg1_price = None;
                        leg1_side = None;
                    }
                }

                _ => {}
            }
        }

        let total_profit = trades.iter().filter(|t| t.profit > 0.0).map(|t| t.profit).sum();
        let total_loss = trades.iter().filter(|t| t.profit < 0.0).map(|t| t.profit.abs()).sum();
        let roi = (balance - self.initial_balance) / self.initial_balance;

        BacktestResult {
            total_cycles: trades.len(),
            successful_cycles: trades.iter().filter(|t| t.completed).count(),
            failed_cycles: trades.iter().filter(|t| !t.completed).count(),
            total_profit,
            total_loss,
            final_balance: balance,
            roi,
            trades,
        }
    }
}
```

---

## 5. 手动模式实现

### 5.1 终端UI设计

```rust
// 使用 crossterm 库实现固定UI
pub struct TerminalUI {
    // 显示当前轮次信息
    // 显示实时价格（UP/DOWN）
    // 显示策略状态
    // 显示仓位信息
}

// 命令解析
pub enum Command {
    BuyUp(f64),           // buy up <usd>
    BuyDown(f64),         // buy down <usd>
    BuySharesUp(u64),     // buyshares up <shares>
    BuySharesDown(u64),   // buyshares down <shares>
    AutoOn {              // auto on <shares> [sum] [move] [windowMin]
        shares: u64,
        sum: f64,
        move_pct: f64,
        window_min: u64,
    },
    AutoOff,              // auto off
    Status,               // status
    Help,                 // help
}
```

### 5.2 手动交易执行

```rust
async fn handle_manual_command(cmd: Command, exec_engine: &BtcExecutionEngine) -> Result<()> {
    match cmd {
        Command::BuySharesUp(shares) => {
            let current_ask = orderbook.get_up_ask();
            exec_engine.execute_manual_buy(
                DumpedSide::Up,
                &up_token_id,
                shares,
                current_ask,
            ).await?;
        }

        Command::AutoOn { shares, sum, move_pct, window_min } => {
            // 启动自动模式
            strategy_config.update(shares, sum, move_pct, window_min);
            state_machine.enable_auto_mode();
        }

        // ... 其他命令
    }
    Ok(())
}
```

---

## 6. 环境变量配置

### 6.1 必需配置

```bash
# Polymarket凭证
POLY_PRIVATE_KEY=0x...
POLY_FUNDER=0x...

# 策略参数
POSITION_SIZE_SHARES=20         # 每条腿的份额数（改名更清晰）
STRATEGY_SUM_TARGET=0.95        # 对冲阈值（默认0.95）
STRATEGY_MOVE_PCT=0.15          # 抛售阈值（默认15%）
STRATEGY_WINDOW_MIN=2           # 时间窗口（默认2分钟）

# 轮次管理
ENABLED_ASSETS=BTC,ETH,SOL      # 监听的资产列表
ROUND_CHECK_INTERVAL_SECS=30    # 轮次检查间隔（秒）

# 系统配置
DRY_RUN=1                       # 1=模拟模式, 0=实盘
RUST_LOG=info
```

### 6.2 可选配置

```bash
# 数据记录
ENABLE_DATA_LOGGING=1           # 启用数据记录
DATA_LOG_PATH=./data/btc_snapshots.jsonl

# 断路器
CB_MAX_POSITION_PER_MARKET=50000
CB_MAX_DAILY_LOSS=500

# 手动模式
MANUAL_MODE=1                   # 启用手动交易
```

---

## 7. 开发计划

### 7.1 第一阶段：核心功能（2周）

- [ ] 实现轮次管理器 (round_manager.rs)
- [ ] 实现抛售检测器 (dump_detector.rs)
- [ ] 实现策略状态机 (strategy_state.rs)
- [ ] 调整执行引擎为单平台顺序执行
- [ ] 集成WebSocket价格流处理
- [ ] 单元测试和集成测试

### 7.2 第二阶段：数据记录与回测（1周）

- [ ] 实现数据记录器 (data_logger.rs)
- [ ] 实现回测引擎 (backtest.rs)
- [ ] 收集测试数据（72小时+）
- [ ] 参数优化测试

### 7.3 第三阶段：手动模式与UI（1周）

- [ ] 实现终端UI
- [ ] 实现命令解析器
- [ ] 实现手动交易执行
- [ ] 实时状态显示

### 7.4 第四阶段：测试与优化（1周）

- [ ] 压力测试
- [ ] 延迟优化
- [ ] 错误处理完善
- [ ] 生产环境部署准备

---

## 8. 风险与限制

### 8.1 技术风险

| 风险 | 影响 | 缓解措施 |
|------|------|----------|
| WebSocket断线 | 错过套利机会 | 自动重连 + 状态保持 |
| 订单部分成交 | 风险敞口 | 自动平仓逻辑 |
| 网络延迟峰值 | 滑点增加 | 断路器保护 |
| 市场流动性不足 | 无法成交 | FAK订单 + 限价保护 |

### 8.2 回测局限性（继承自原策略）

1. **数据采样率**：每秒1次快照，无法捕捉秒内微观波动
2. **订单簿深度**：未建模可用成交量
3. **滑点模型**：恒定滑点，实际可变（200-1500ms）
4. **市场影响**：假设为价格接受者，未建模自身订单对市场的影响
5. **费用简化**：Polymarket零费用，但可能有gas费

### 8.3 策略假设

1. **抛售持续性**：假设快速抛售后价格会稳定
2. **均值回归**：假设价格会回归合理区间（sumTarget可达成）
3. **市场效率**：假设市场不会长期维持 `UP + DOWN < 1.00` 的极端状况

---

## 9. 性能优化方向

### 9.1 已实现优化（继承自现有代码）

- ✅ 无锁原子订单簿（AtomicU64打包）
- ✅ SIMD加速（虽然单市场可能不需要）
- ✅ 异步批处理（仓位更新）
- ✅ 连接池复用

### 9.2 未来优化

1. **更快的RPC节点**：部署专用Polygon RPC减少延迟
2. **地理位置优化**：VPS靠近Polymarket服务器
3. **Rust重写Python客户端**：已完成（polymarket_clob.rs）
4. **预签名订单**：提前准备签名以减少执行延迟

---

## 10. 总结

### 10.1 可复用组件占比

- **直接复用**：约 60%（Polymarket客户端、订单执行、风险管理）
- **调整复用**：约 20%（执行引擎、仓位跟踪、类型系统）
- **新增开发**：约 20%（轮次管理、抛售检测、策略状态机、回测）

### 10.2 技术栈

- **语言**：Rust（主要）+ Python（可选工具脚本）
- **异步运行时**：Tokio
- **WebSocket**：tokio-tungstenite
- **HTTP客户端**：reqwest
- **以太坊**：ethers-rs
- **数据序列化**：serde_json
- **终端UI**：crossterm（可选）

### 10.3 核心优势

1. **高性能基础**：继承现有代码的SIMD、无锁设计
2. **成熟的风险管理**：断路器、仓位限制、自动平仓
3. **生产级稳定性**：自动重连、错误恢复、持久化
4. **完整的回测框架**：支持策略验证和参数优化
5. **灵活的交易模式**：支持手动和自动两种模式

### 10.4 预期成果

- **实时交易机器人**：支持BTC 15分钟市场的自动化策略
- **回测系统**：可重放历史数据并优化参数
- **手动交易工具**：实时UI + 命令行操作
- **数据收集器**：持续积累历史数据集

---

## 附录 A：参数调优建议

基于原策略的回测结果，建议的参数范围：

| 参数 | 保守值 | 激进值 | 说明 |
|------|--------|--------|------|
| shares | 20 | 50 | 仓位大小 |
| sum_target | 0.95 | 0.60 | 对冲阈值（越低越激进）|
| move_pct | 0.15 (15%) | 0.01 (1%) | 抛售阈值（越低越激进）|
| window_min | 2 | 15 | 时间窗口（越长越激进）|

**保守参数集**（ROI 86%，4天）：
```
shares = 20
sum_target = 0.95
move_pct = 0.15
window_min = 2
```

**激进参数集**（ROI -50%，2天）：
```
shares = 20
sum_target = 0.60
move_pct = 0.01
window_min = 15
```

**建议**：从保守参数开始测试，逐步调整。

---

## 附录 B：部署检查清单

- [ ] Polymarket钱包已充值USDC
- [ ] 环境变量已配置
- [ ] 断路器限制已设置
- [ ] DRY_RUN=1 模式测试通过
- [ ] 回测数据收集完成（至少72小时）
- [ ] 网络延迟测试（ping Polymarket API）
- [ ] 日志系统正常
- [ ] 仓位跟踪文件路径可写
- [ ] 监控告警已配置（可选）

---

---

## 附录 C：未对冲风险管理方案

当第一阶段执行后，如果对冲条件一直不满足，采用以下风险管理策略。

**实施策略**：
- ✅ **第一阶段实现**：硬止损（方案1） - 保底保护，防止爆仓
- 🔄 **未来扩展**：动态阈值（方案2）、时间加权（方案3） - 可选优化

### 方案1：硬止损 ✅ 优先实现

**逻辑**：设置浮亏阈值，触发后强制平仓止损。

**⚠️ 重要**：在轮次结束前30秒，即使未触发止损阈值，也应强制平仓，避免轮次结束时订单无法成交。

```rust
pub struct RiskConfig {
    pub stop_loss_enabled: bool,    // 是否启用硬止损（必须显式检查）
    pub stop_loss_pct: f64,         // 硬止损阈值（默认0.50 = 50%）
    pub adaptive_threshold: bool,   // 是否启用动态阈值
    pub force_hedge_secs: u64,      // 强制对冲时间（收盘前30秒）
}

impl StrategyStateMachine {
    pub fn check_stop_loss(&self, current_price: u16) -> bool {
        // 必须检查是否启用
        if !self.config.stop_loss_enabled {
            return false;
        }
        let leg1_price = ctx.leg1_entry_price.unwrap();
        let side = ctx.dumped_side.unwrap();

        // 计算浮动亏损
        let unrealized_loss = match side {
            DumpedSide::Up => {
                // 买入UP @ 45¢，现在跌至25¢
                if current_price < leg1_price {
                    (leg1_price - current_price) as f64 / leg1_price as f64
                } else {
                    0.0
                }
            }
            DumpedSide::Down => {
                if current_price < leg1_price {
                    (leg1_price - current_price) as f64 / leg1_price as f64
                } else {
                    0.0
                }
            }
        };

        if unrealized_loss >= self.config.stop_loss_pct {
            warn!("🛑 触发硬止损: 浮亏{:.2}% (阈值{:.2}%)",
                  unrealized_loss * 100.0, self.config.stop_loss_pct * 100.0);
            return true;
        }
        false
    }
}
```

**示例**：
```
买入UP @ 45¢
止损阈值：50%
触发价格：45 × (1 - 0.50) = 22.5¢

价格跌至22¢ → 触发止损 → 强制卖出
损失：45 - 22 = 23¢（控制在50%以内）
```

---

### 方案2：动态阈值放宽 🔄 未来扩展

**逻辑**：随着等待时间推移，逐渐放宽 `sum_target`，增加对冲机会。

**注**：暂不实现，留作未来优化选项。

```rust
impl StrategyStateMachine {
    pub fn adaptive_sum_target(&self, elapsed_secs: u64) -> f64 {
        if !self.config.adaptive_threshold {
            return self.config.sum_target;  // 禁用时使用固定阈值
        }

        let base = self.config.sum_target;  // 初始阈值 0.95
        let max = 0.99;                     // 最宽松阈值
        let ramp_time = 300;                // 5分钟内完成放宽

        let progress = (elapsed_secs as f64 / ramp_time as f64).min(1.0);
        let current_threshold = base + (max - base) * progress;

        current_threshold
    }

    pub fn check_adaptive_hedge(&self, up_ask: u16, down_ask: u16, elapsed_secs: u64) -> bool {
        let leg1_price = ctx.leg1_entry_price.unwrap();
        let opposite_ask = match ctx.dumped_side.unwrap() {
            DumpedSide::Up => down_ask,
            DumpedSide::Down => up_ask,
        };

        let total_cost = leg1_price + opposite_ask;
        let threshold = self.adaptive_sum_target(elapsed_secs);
        let threshold_cents = (threshold * 100.0) as u16;

        if total_cost <= threshold_cents {
            info!("✅ 动态对冲: {}¢ <= {}¢ (等待{}秒后放宽)",
                  total_cost, threshold_cents, elapsed_secs);
            return true;
        }
        false
    }
}
```

**时间线示例**：
```
买入UP @ 45¢
初始阈值：sum_target = 0.95

T+0s:   需要 DOWN <= 50¢ (45+50=95)
T+150s: 需要 DOWN <= 52¢ (45+52=97, 阈值放宽至0.97)
T+300s: 需要 DOWN <= 54¢ (45+54=99, 阈值放宽至0.99)

利润变化：
- T+0s 对冲：赚 5¢
- T+150s 对冲：赚 3¢
- T+300s 对冲：赚 1¢
```

---

### 方案3：时间加权强制对冲 🔄 未来扩展

**逻辑**：距离收盘越近，越激进地对冲（容忍更低利润甚至小亏）。

**注**：暂不实现，留作未来优化选项。

**⚠️ 重要**：如果实现此方案，需要与轮次切换协调：
- 时间加权应在轮次结束前触发（如最后30秒）
- 轮次切换应等到轮次真正结束（endDate），不提前
- 避免"提前切换"与"强制对冲"冲突

```rust
impl StrategyStateMachine {
    pub fn time_weighted_hedge(&self, up_ask: u16, down_ask: u16, remaining_secs: u64) -> bool {
        let leg1_price = ctx.leg1_entry_price.unwrap();
        let opposite_ask = match ctx.dumped_side.unwrap() {
            DumpedSide::Up => down_ask,
            DumpedSide::Down => up_ask,
        };

        let total_cost = leg1_price + opposite_ask;

        // 根据剩余时间动态调整阈值
        let threshold = if remaining_secs > 600 {
            95  // 10分钟以上：严格阈值
        } else if remaining_secs > 300 {
            98  // 5-10分钟：放宽阈值
        } else if remaining_secs > 120 {
            99  // 2-5分钟：接受微利
        } else if remaining_secs > 30 {
            100 // 30秒-2分钟：保本即可
        } else {
            105 // 最后30秒：接受小亏（最多5¢）
        };

        if total_cost <= threshold {
            warn!("⏰ 时间紧迫对冲: {}¢ <= {}¢ (剩余{}秒)",
                  total_cost, threshold, remaining_secs);
            return true;
        }
        false
    }
}
```

**场景示例**：
```
买入UP @ 45¢
对方(DOWN)价格: 56¢
总成本: 101¢（正常阈值95¢不满足）

剩余10分钟：不对冲（101 > 95）
剩余2分钟：不对冲（101 > 99）
剩余1分钟：不对冲（101 > 100）
剩余20秒：强制对冲（101 <= 105）

结果：小亏1¢，避免单边暴露清零风险
```

---

## 风险管理集成逻辑

### 当前实现（MVP版本）

```rust
async fn run_btc_websocket(...) -> Result<()> {
    loop {
        let price_update = ws.recv().await?;
        let current_price = parse_price(price_update);

        // 优先级0：检查轮次是否已结束 ⚠️ 最高优先级
        if round_manager.is_round_ended(&asset) {
            let ctx = state_machine.context.lock().unwrap();

            // 如果有未对冲的仓位
            if ctx.state == StrategyState::WaitingForHedge {
                warn!("⚠️ 轮次结束，第一阶段未对冲，视为损失");
                // 记录损失（根据 bot_cn.md 保守假设）
                position_tracker.record_uncompleted_cycle(
                    ctx.leg1_entry_price.unwrap(),
                    ctx.leg1_fill_size.unwrap(),
                );
            }
            drop(ctx);

            // 切换到下一轮次
            let new_round = round_manager.fetch_next_round(&asset).await?;
            info!("🔄 切换到新轮次: {}", new_round.round_slug);

            // 重置状态机
            state_machine.reset_for_new_round(
                new_round.round_slug.clone(),
                new_round.start_timestamp * 1_000_000_000,
                config.window_min,
            );

            // 重新订阅 WebSocket（使用新的 token IDs）
            ws_reconnect(&new_round.up_token_id, &new_round.down_token_id).await?;
            continue;
        }

        // 优先级1：提前平仓（价格反弹10%）
        if state_machine.check_early_exit(current_price) {
            info!("💰 提前平仓：价格反弹触发");
            exec_engine.sell_position(...).await?;
            state_machine.complete_cycle();
            continue;
        }

        // 优先级2：轮次结束前强制平仓（避免无法成交）⚠️ 新增
        let remaining_secs = round_manager.get_remaining_seconds(&asset).unwrap_or(0);
        if remaining_secs < 30 && ctx.state == StrategyState::WaitingForHedge {
            warn!("⏰ 轮次即将结束（剩余{}秒），强制平仓第一阶段", remaining_secs);
            exec_engine.sell_position(...).await?;
            state_machine.complete_cycle();
            continue;
        }

        // 优先级3：硬止损（浮亏50%）✅ 当前实现
        if state_machine.check_stop_loss(current_price) {
            warn!("🛑 硬止损：浮亏超过阈值");
            exec_engine.sell_position(...).await?;
            state_machine.complete_cycle();
            continue;
        }

        // 优先级4：正常对冲条件
        let (up_ask, down_ask) = orderbook.get_both_asks();
        if state_machine.check_hedge_condition(up_ask, down_ask) {
            info!("💰 正常对冲：满足sum_target条件");
            exec_engine.execute_leg2(...).await?;
            state_machine.complete_cycle();
            continue;
        }

        // 默认：继续等待
    }
}
```

### 未来扩展（可选）

如果需要进一步优化对冲成功率，可添加：

```rust
// 优先级3：时间加权强制对冲（收盘前30秒）🔄 未来扩展
if state_machine.time_weighted_hedge(up_ask, down_ask, remaining_secs) {
    exec_engine.execute_leg2(...).await?;
    state_machine.complete_cycle();
    continue;
}

// 优先级4：动态阈值对冲（5分钟渐进放宽）🔄 未来扩展
if state_machine.check_adaptive_hedge(up_ask, down_ask, elapsed_secs) {
    exec_engine.execute_leg2(...).await?;
    state_machine.complete_cycle();
    continue;
}
```

---

## 配置参数

### 环境变量

**当前实现**：
```bash
# 提前平仓
EARLY_EXIT_ENABLED=false        # 是否启用提前平仓（默认关闭）
EARLY_EXIT_PROFIT_PCT=0.10      # 反弹10%获利

# 硬止损 ✅
STOP_LOSS_ENABLED=true          # 启用硬止损（必须显式检查）
STOP_LOSS_PCT=0.50              # 浮亏50%止损
```

**未来扩展**（暂不配置）：
```bash
# 动态阈值 🔄
ADAPTIVE_THRESHOLD=false        # 暂不启用
ADAPTIVE_RAMP_SECS=300

# 时间加权 🔄
FORCE_HEDGE_ENABLED=false       # 暂不启用
FORCE_HEDGE_BEFORE_SECS=30
```

### 决策树

**当前实现（MVP版本）**：
```
未对冲时的处理流程：

优先级0: 轮次是否已结束？→ 是 → 记录未对冲损失，切换到下一轮次 🔄
    ↓ 否
优先级1: 价格反弹 >= 10% → 提前平仓 💰 利润最大化（可选）
    ↓ 否
优先级2: 剩余时间 < 30秒 → 强制平仓 ⏰ 避免无法成交 ✅
    ↓ 否
优先级3: 浮亏 >= 50% → 硬止损 🛑 防止爆仓 ✅
    ↓ 否
优先级4: 满足正常阈值(sum_target) → 正常对冲 💰 标准套利
    ↓ 否
继续等待 ⏳
```

**未来扩展（可选）**：
```
满足正常阈值 → 正常对冲 💰
    ↓ 否
距收盘 < 30秒 → 强制对冲 ⏰ 🔄 未来扩展
    ↓ 否
等待时间长 → 动态阈值对冲 📈 🔄 未来扩展
    ↓ 否
继续等待 ⏳
```

---

## 附录 D：常见问题 FAQ

### Q1: DumpDetector 是用死循环实现的吗？

**A**: 不是。DumpDetector 是**事件驱动**设计，不需要独立线程或循环。

工作方式：
```rust
// WebSocket主循环（唯一循环）
loop {
    let price_update = ws.recv().await?;  // 等待服务器推送

    // 被动调用检测（不是循环）
    if dump_detector.check_dump_up(price, now) {
        trigger_leg1();
    }
}
```

优势：
- ✅ 只在价格更新时计算，不空转
- ✅ 低延迟，无轮询开销
- ✅ 自动清理历史数据（只保留3秒窗口）

---

### Q2: 推荐的服务器配置是什么？

**A**: 根据资金规模选择：

| 场景 | 配置 | 服务商 | 月费 | 延迟 |
|------|------|--------|------|------|
| **测试学习** | 2核2GB | DigitalOcean NYC | $12 | 良好 |
| **小资金实盘** | 2核2GB | AWS t3.small (us-east-1) | $15 | 优秀 |
| **推荐生产** | 2核4GB | AWS t3.medium (us-east-1) | $30 | 优秀 |
| **极致成本** | 树莓派4B | 自建 | ~$2 | 中等 |

**关键因素**：
1. **地理位置**：优先选择美国东海岸（us-east-1/纽约），靠近 Polymarket 服务器
2. **网络延迟**：目标 < 100ms 到 `clob.polymarket.com`
3. **稳定性** > 硬件性能（Rust程序对资源要求低）

**部署前测试**：
```bash
# 测试关键API延迟
ping clob.polymarket.com
curl -w "Total: %{time_total}s\n" https://clob.polymarket.com
```

---

### Q3: Polygon RPC 节点是必需的吗？

**A**: **不需要**。当前项目使用 Polymarket CLOB API 模式，不直接上链。

架构说明：
```
你的机器人 → Polymarket CLOB API → 订单撮合
               (HTTPS + WebSocket)
                      ↓
            定期批量上链（Polymarket处理）
```

**真正需要的网络连接**：
- ✅ `clob.polymarket.com` - 订单提交（必需）
- ✅ WebSocket 价格流 - 实时数据（必需）
- 🟡 `gamma-api.polymarket.com` - 市场发现（可选）
- ❌ Polygon RPC - 不需要

**EIP-712 签名**在本地完成，不需要调用 RPC。

**什么情况需要 Polygon RPC？**
- 直接调用链上智能合约（本项目不涉及）
- 实时监听链上事件（CLOB模式已提供）
- 查询钱包余额（可选，可通过 CLOB API 查询）

因此**无需购买专用 RPC 节点**，节省成本。

---

**文档版本**：v1.1
**最后更新**：2026-01-01
**作者**：Claude Code
**审核状态**：待审核
