#!/bin/bash

# Up/Down 交易策略运行脚本
source .env
# 检查必需的环境变量
if [ -z "$POLY_PRIVATE_KEY" ]; then
    echo "错误: 缺少 POLY_PRIVATE_KEY 环境变量"
    echo "用法: POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... ./run_updown_trader.sh"
    exit 1
fi

if [ -z "$POLY_FUNDER" ]; then
    echo "错误: 缺少 POLY_FUNDER 环境变量"
    echo "用法: POLY_PRIVATE_KEY=0x... POLY_FUNDER=0x... ./run_updown_trader.sh"
    exit 1
fi

# 默认配置（可通过环境变量覆盖）
export DRY_RUN=${DRY_RUN:-1}
export ENABLED_ASSETS=${ENABLED_ASSETS:-BTC}
export POSITION_SIZE_SHARES=${POSITION_SIZE_SHARES:-20}
export STRATEGY_SUM_TARGET=${STRATEGY_SUM_TARGET:-0.95}
export STRATEGY_MOVE_PCT=${STRATEGY_MOVE_PCT:-0.15}
export STRATEGY_WINDOW_SECS=${STRATEGY_WINDOW_SECS:-3}
export STOP_LOSS_ENABLED=${STOP_LOSS_ENABLED:-true}
export STOP_LOSS_PCT=${STOP_LOSS_PCT:-0.50}
export EARLY_EXIT_ENABLED=${EARLY_EXIT_ENABLED:-false}
export EARLY_EXIT_PROFIT_PCT=${EARLY_EXIT_PROFIT_PCT:-0.10}
export DATA_DIR=${DATA_DIR:-./data}
export RUST_LOG=${RUST_LOG:-info}

# 创建数据目录
mkdir -p $DATA_DIR

echo "========================================="
echo "  Up/Down 交易策略启动"
echo "========================================="
echo "模式: $([ "$DRY_RUN" = "1" ] && echo "模拟（真实数据 + 假下单）" || echo "真实交易 ⚠️")"
echo "资产: $ENABLED_ASSETS"
echo "仓位: $POSITION_SIZE_SHARES 份"
echo "对冲目标: $STRATEGY_SUM_TARGET"
echo "抛售阈值: $(echo "$STRATEGY_MOVE_PCT * 100" | bc)%"
echo "止损阈值: $(echo "$STOP_LOSS_PCT * 100" | bc)%"
echo "数据目录: $DATA_DIR"
echo "========================================="

# 编译（如果需要）
if [ ! -f "target/release/updown_trader" ]; then
    echo "🔨 编译主程序..."
    cargo build --release --bin updown_trader
    if [ $? -ne 0 ]; then
        echo "❌ 编译失败"
        exit 1
    fi
fi

# 运行
echo "🚀 启动交易程序..."
./target/release/updown_trader
