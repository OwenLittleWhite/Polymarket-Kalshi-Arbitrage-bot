#!/bin/bash

# 测试 Polymarket 官方 SDK 认证脚本

set -e

echo "检查依赖..."
if [ ! -d "node_modules" ]; then
    echo "安装 npm 依赖..."
    npm install
fi

echo ""
echo "运行官方 SDK 认证测试..."
echo ""

# 从 .env 读取环境变量（如果存在）
if [ -f .env ]; then
    echo "从 .env 加载环境变量..."
    export $(grep -v '^#' .env | xargs)
fi

# 检查必需的环境变量
if [ -z "$PRIVATE_KEY" ] && [ -z "$POLY_PRIVATE_KEY" ]; then
    echo "错误: 需要设置 PRIVATE_KEY 或 POLY_PRIVATE_KEY 环境变量"
    echo ""
    echo "用法:"
    echo "  PRIVATE_KEY=0x... ./test_auth_sdk.sh"
    echo ""
    echo "或在 .env 文件中设置:"
    echo "  PRIVATE_KEY=0x..."
    exit 1
fi

# 运行 TypeScript 脚本（使用 npx，不需要全局安装 ts-node）
npx ts-node test_polymarket_auth.ts
