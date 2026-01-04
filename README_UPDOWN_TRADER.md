# Up/Down 交易策略 - 使用指南

## 快速开始

### 1. 编译程序

```bash
cargo build --release --bin updown_trader
```

### 2. 配置环境变量

创建 `.env` 文件或直接导出环境变量：

```bash
# 必需 - Polymarket 凭证
export POLY_PRIVATE_KEY="0x..."
export POLY_FUNDER="0x..."

# 可选 - 策略参数（以下为默认值）
export DRY_RUN=1                      # 1=模拟模式, 0=真实交易
export ENABLED_ASSETS=BTC             # 支持多资产: BTC,ETH,SOL
export POSITION_SIZE_SHARES=20        # 每次交易份额
export STRATEGY_SUM_TARGET=0.95       # 对冲目标
export STRATEGY_MOVE_PCT=0.15         # 抛售阈值 (15%)
export STRATEGY_WINDOW_SECS=3         # 检测窗口 (3秒)
export STOP_LOSS_ENABLED=true         # 启用止损
export STOP_LOSS_PCT=0.50             # 止损阈值 (50%)
export EARLY_EXIT_ENABLED=false       # 早期退出（可选）
export EARLY_EXIT_PROFIT_PCT=0.10     # 早期退出阈值 (10%)
export DATA_DIR=./data                # 数据存储目录
export RUST_LOG=info                  # 日志级别
```

### 3. 运行程序

**方式 1: 使用运行脚本（推荐）**

```bash
POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... ./run_updown_trader.sh
```

**方式 2: 直接运行**

```bash
# 模拟模式
DRY_RUN=1 POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... \
  cargo run --release --bin updown_trader

# 真实交易（谨慎！）
DRY_RUN=0 POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... \
  cargo run --release --bin updown_trader
```

---

## 工作原理

### 策略流程

```
1. 连接 WebSocket → 接收实时价格
2. 检测抛售 → 买入单侧（Phase 1）
3. 等待对冲条件 → 买入对侧（Phase 2）或止损/退出
4. 轮次结束 → 保存数据，切换到下一轮
```

### 主循环优先级

程序按以下优先级处理：

```
Priority 0: 轮次结束? → 切换轮次
Priority 1: 价格反弹 10%? → 早期退出
Priority 2: < 30秒剩余? → 强制平仓
Priority 3: 亏损 >= 50%? → 硬止损
Priority 4: 对冲条件满足? → 正常对冲
Priority 5: 继续等待
```

### 模拟模式说明

**DRY_RUN=1（模拟模式）**：
- ✅ 连接真实 WebSocket
- ✅ 接收真实价格数据
- ✅ 运行完整策略逻辑
- ✅ 检查真实订单簿流动性
- ❌ 但不调用下单 API
- ✅ 记录模拟成交到 JSON 文件

**DRY_RUN=0（真实交易）**：
- ✅ 所有功能同上
- ⚠️  **实际调用 buy_fak/sell_fak API**
- ⚠️  **真实资金交易，请谨慎！**

---

## 数据存储

### 文件格式

程序会在 `DATA_DIR` 目录下创建 JSON 文件：

```
data/
├── BTC_20260101101500.json    # 2026-01-01 10:15 轮次
├── BTC_20260101103000.json    # 2026-01-01 10:30 轮次
└── ETH_20260101101500.json    # ETH 轮次
```

### 数据结构

```json
{
  "asset": "BTC",
  "slug": "btc-updown-15m-1767262500",
  "up_token_id": "108702...",
  "down_token_id": "14771...",
  "start_timestamp": 1767262500,
  "end_timestamp": 1767263400,
  "price_snapshots": [
    {
      "timestamp_ns": 1000000000,
      "up_ask": 5000,
      "down_ask": 5000,
      "up_liquidity": 100.0,
      "down_liquidity": 100.0
    }
  ],
  "trades": [
    {
      "timestamp_ns": 2000000000,
      "action": "buy",
      "side": "up",
      "token_id": "token_up",
      "price": 0.50,
      "size": 20.0,
      "is_simulated": true
    }
  ]
}
```

---

## 日志示例

### 启动日志

```
🚀 Up/Down 交易策略启动
⚙️  模式: 模拟（真实数据 + 假下单）
⚙️  资产: ["BTC"]
⚙️  仓位大小: 20 份
⚙️  对冲目标: 0.95
⚙️  抛售阈值: 15%
⚙️  止损阈值: 50%
✅ Polymarket 客户端初始化完成
📊 [BTC] 当前轮次: btc-updown-15m-1767262500
    Up Token: 108702...
    Down Token: 14771...
✅ 策略模块初始化完成
🔌 连接 Polymarket WebSocket...
📡 [BTC] 订阅 WebSocket: 108702... / 14771...
✅ WebSocket 连接成功，开始监听价格...
```

### 交易日志

```
🔔 检测到抛售: Up 侧
✅ Phase 1 买入成功: 20.00 份 @ $0.5000
💾 保存数据: BTC_20260101101500.json (45 个价格快照, 1 笔交易)

✅ 对冲条件满足
✅ Phase 2 对冲成功: 20.00 份 @ $0.4500
💾 保存数据: BTC_20260101101500.json (89 个价格快照, 2 笔交易)
```

### 止损日志

```
🛑 触发止损（亏损 >= 50%）
✅ stop_loss 成功: 20.00 份 @ $0.2500
💾 保存数据: BTC_20260101101500.json (120 个价格快照, 2 笔交易)
```

### 轮次切换日志

```
🔄 轮次结束: btc-updown-15m-1767262500
💾 保存数据: BTC_20260101101500.json (450 个价格快照, 2 笔交易)
✅ 切换到新轮次: btc-updown-15m-1767263400
```

---

## 监控和调试

### 查看实时日志

```bash
# 基础日志
RUST_LOG=info ./target/release/updown_trader

# 详细调试日志
RUST_LOG=debug ./target/release/updown_trader

# 只看特定模块
RUST_LOG=updown_trader=debug ./target/release/updown_trader
```

### 查看数据文件

```bash
# 列出所有轮次数据
ls -lh data/

# 查看最新轮次
cat data/BTC_*.json | jq .

# 统计交易数量
cat data/BTC_*.json | jq '.trades | length'

# 查看所有交易
cat data/BTC_*.json | jq '.trades[]'
```

---

## 常见问题

### Q: 如何测试程序是否正常运行？

**A**: 使用模拟模式运行 5-10 分钟，查看数据目录是否生成 JSON 文件。

```bash
DRY_RUN=1 POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... \
  timeout 600 ./target/release/updown_trader

# 检查数据
ls -lh data/
cat data/BTC_*.json | jq '.price_snapshots | length'
```

### Q: WebSocket 连接失败怎么办？

**A**: 检查网络连接和 Polymarket API 状态：

```bash
# 测试 WebSocket 连接
wscat -c wss://ws-subscriptions-clob.polymarket.com/ws/market

# 测试 Gamma API
curl https://gamma-api.polymarket.com/markets?slug=btc-updown-15m-$(date +%s)
```

### Q: 如何验证策略逻辑？

**A**: 查看数据文件中的交易记录：

```bash
# 查看是否有 Phase 1 买入
cat data/BTC_*.json | jq '.trades[] | select(.action == "buy" and .is_simulated == true)'

# 查看是否有 Phase 2 对冲
cat data/BTC_*.json | jq '.trades | length' # 应该是偶数（buy + hedge）
```

### Q: 止损是否生效？

**A**: 检查日志中的止损触发消息：

```bash
# 运行时过滤止损日志
./target/release/updown_trader 2>&1 | grep "止损"
```

### Q: 如何切换到真实交易？

**A**: ⚠️ 请先在模拟模式下运行 24-72 小时，确认策略稳定后再切换：

```bash
# 1. 先模拟至少 24 小时
DRY_RUN=1 ... ./target/release/updown_trader

# 2. 分析数据，确认策略逻辑正确

# 3. 谨慎切换到真实交易
DRY_RUN=0 ... ./target/release/updown_trader
```

---

## 性能优化

### 减少日志输出

```bash
# 只显示警告和错误
RUST_LOG=warn ./target/release/updown_trader
```

### 调整数据保存频率

修改 `src/bin/updown_trader.rs`：

```rust
// 当前：每 100 个价格快照保存一次（约 5-10 秒）
if snapshot.price_snapshots.len() % 100 == 0 {

// 改为：每 500 个保存一次（约 30-60 秒）
if snapshot.price_snapshots.len() % 500 == 0 {
```

---

## 安全建议

1. **永远先用模拟模式测试** - 至少运行 24 小时
2. **检查 API 密钥权限** - 确保 funder 地址有足够余额
3. **监控资金变化** - 真实交易时密切关注账户余额
4. **设置合理仓位** - 建议从小仓位（10-20 份）开始
5. **启用止损保护** - 必须设置 `STOP_LOSS_ENABLED=true`
6. **定期备份数据** - 数据文件包含所有交易记录

---

## 技术支持

- 技术文档: `IMPLEMENTATION_SUMMARY.md`
- 策略说明: `bot_cn.md`
- 技术方案: `bot_plan.md`
- 源代码: `src/bin/updown_trader.rs`

---

**最后更新**: 2026-01-01
**版本**: 1.0.0
