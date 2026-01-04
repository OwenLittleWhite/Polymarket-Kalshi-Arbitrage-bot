# Up/Down 交易策略 - 快速开始

## 一键启动（模拟模式）

```bash
# 1. 设置环境变量
export POLY_PRIVATE_KEY="你的私钥"
export POLY_FUNDER="你的 funder 地址"

# 2. 运行（推荐先模拟 24 小时）
./run_updown_trader.sh
```

## 检查运行状态

```bash
# 查看生成的数据文件
ls -lh data/

# 查看最新轮次数据
cat data/BTC_*.json | jq .

# 统计交易数量
cat data/BTC_*.json | jq '.trades | length'
```

## 详细文档

- **使用指南**: `README_UPDOWN_TRADER.md`
- **技术总结**: `IMPLEMENTATION_SUMMARY.md`
- **策略说明**: `bot_cn.md`

## 重要提示

⚠️ **请务必先在模拟模式下运行 24-72 小时，确认策略逻辑正确后再切换到真实交易！**

```bash
# 模拟模式（默认）
DRY_RUN=1 ./run_updown_trader.sh

# 真实交易（谨慎！）
DRY_RUN=0 ./run_updown_trader.sh
```
