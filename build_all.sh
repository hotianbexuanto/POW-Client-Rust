#!/bin/bash
# 构建所有平台的二进制文件

set -e

echo "===== 开始多平台构建 ====="

# 确保目标目录存在
mkdir -p ./dist

# 1. 构建Windows x86_64版本
echo "正在构建Windows x86_64版本..."
cargo build --release --target x86_64-pc-windows-msvc
cp target/x86_64-pc-windows-msvc/release/pow-client.exe ./dist/

# 2. 构建Linux x86_64版本
echo "正在构建Linux x86_64版本..."
cargo build --release --target x86_64-unknown-linux-gnu
cp target/x86_64-unknown-linux-gnu/release/pow-client ./dist/pow-client-linux-x86_64

# 3. 构建Linux ARM64版本
echo "正在构建Linux ARM64版本..."
cargo build --release --target aarch64-unknown-linux-gnu
cp target/aarch64-unknown-linux-gnu/release/pow-client ./dist/pow-client-linux-arm64

# 4. 构建Termux (Android ARM64)版本
# 注意：需要安装Android NDK和相关工具链
echo "正在构建Termux ARM64版本..."
if command -v aarch64-linux-android21-clang &> /dev/null; then
  # 检查是否已添加目标
  if ! rustup target list --installed | grep -q "aarch64-linux-android"; then
    echo "添加aarch64-linux-android目标..."
    rustup target add aarch64-linux-android
  fi
  
  # 设置Android NDK环境变量（如果未设置）
  if [ -z "$ANDROID_NDK_HOME" ]; then
    echo "警告: ANDROID_NDK_HOME环境变量未设置，构建可能会失败"
    echo "请先设置ANDROID_NDK_HOME指向Android NDK路径"
  else
    export PATH=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin:$PATH
    env CC=aarch64-linux-android21-clang CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=aarch64-linux-android21-clang cargo build --release --target aarch64-linux-android
    cp target/aarch64-linux-android/release/pow-client ./dist/pow-client-termux-arm64
  fi
else
  echo "警告: 未安装Android NDK工具链，跳过Termux版本构建"
  echo "请安装Android NDK并确保aarch64-linux-android21-clang在PATH中可用"
fi

echo "===== 构建完成 ====="
echo "所有二进制文件已保存到 ./dist 目录"
ls -la ./dist 