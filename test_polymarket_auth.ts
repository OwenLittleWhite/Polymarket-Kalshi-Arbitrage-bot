#!/usr/bin/env ts-node

/**
 * 使用官方 @polymarket/clob-client 测试 Polymarket API 认证
 *
 * 安装依赖:
 *   npm install @polymarket/clob-client ethers@5.8.0
 *   npm install -g ts-node typescript @types/node
 *
 * 运行:
 *   PRIVATE_KEY=0x... ts-node test_polymarket_auth.ts
 *
 * 或使用 .env:
 *   source .env && ts-node test_polymarket_auth.ts
 */

import { ClobClient } from "@polymarket/clob-client";
import { Wallet } from "ethers";

// 配置
const HOST = "https://clob.polymarket.com";
const CHAIN_ID = 137; // Polygon mainnet

// 获取并验证私钥
const privateKeyFromEnv = process.env.PRIVATE_KEY || process.env.POLY_PRIVATE_KEY;
if (!privateKeyFromEnv) {
    console.error('❌ 错误: 需要设置 PRIVATE_KEY 或 POLY_PRIVATE_KEY 环境变量');
    console.error('用法: PRIVATE_KEY=0x... ts-node test_polymarket_auth.ts');
    process.exit(1);
}
const PRIVATE_KEY: string = privateKeyFromEnv;

async function main() {
    console.log('='.repeat(60));
    console.log('Polymarket 官方 SDK 认证测试');
    console.log('='.repeat(60));
    console.log();

    try {
        // 1. 创建 Signer
        console.log('✓ 创建钱包 Signer...');
        const signer = new Wallet(PRIVATE_KEY);
        const walletAddress = signer.address;

        console.log(`  钱包地址: ${walletAddress}`);
        console.log(`  私钥: ${PRIVATE_KEY.substring(0, 10)}...`);
        console.log(`  Chain ID: ${CHAIN_ID}`);
        console.log(`  API Host: ${HOST}`);
        console.log();

        // 2. 创建 ClobClient
        console.log('✓ 创建 ClobClient (官方 SDK)...');
        const client = new ClobClient(
            HOST,
            CHAIN_ID,
            signer  // Signer enables L1 methods
        );
        console.log('  客户端初始化成功');
        console.log();

        // 3. 调用 createOrDeriveApiKey
        console.log('✓ 调用 createOrDeriveApiKey()...');
        console.log('  (这会自动处理 EIP-712 签名和 API 认证)');
        console.log();

        const apiCreds = await client.createOrDeriveApiKey();

        // 4. 成功
        console.log('✅ 成功！获取到 API 凭证:');
        console.log();
        console.log('API Credentials:');
        console.log(JSON.stringify(apiCreds, null, 2));
        console.log();
        console.log('='.repeat(60));
        console.log('🎉 认证成功！你的钱包可以正常使用 Polymarket API。');
        console.log('='.repeat(60));
        console.log();
        console.log('说明:');
        console.log(`  - apiKey: 用于后续 API 调用的认证`);
        console.log(`  - secret: API 密钥`);
        console.log(`  - passphrase: 额外的口令保护`);
        console.log();
        console.log('这些凭证可以保存并在后续请求中使用，避免每次都重新签名。');
        console.log();

    } catch (error: any) {
        console.log('❌ 失败!');
        console.log();

        if (error.response) {
            // HTTP 错误
            console.log('HTTP 错误:');
            console.log(`  状态码: ${error.response.status}`);
            console.log(`  响应: ${JSON.stringify(error.response.data, null, 2)}`);
            console.log();

            if (error.response.status === 400) {
                console.log('可能的原因:');
                console.log(`  1. 钱包地址未在 Polymarket 注册`);
                console.log(`  2. 需要先访问 https://polymarket.com 登录一次`);
                console.log(`  3. 可能需要完成 KYC 或其他账户激活步骤`);
                console.log();
                console.log('建议操作:');
                console.log(`  1. 用 MetaMask 导入私钥: ${PRIVATE_KEY.substring(0, 10)}...`);
                console.log(`  2. 访问 https://polymarket.com 并用该钱包连接`);
                console.log(`  3. 确认能在网站上正常浏览和交易`);
                console.log(`  4. 再重新运行此脚本`);
            }
        } else if (error.message) {
            console.log('错误信息:', error.message);
            console.log();
            console.log('完整错误:');
            console.log(error);
        } else {
            console.log('未知错误:');
            console.log(error);
        }

        console.log();
        console.log('='.repeat(60));
        process.exit(1);
    }
}

main().catch(error => {
    console.error('程序异常:', error);
    process.exit(1);
});
