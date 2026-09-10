#!/usr/bin/env bash
# Build Mesa Intel Vulkan HAL driver for ARO (x86_64-linux-android37)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "${SCRIPT_DIR}/.." && pwd)"
NDK_DIR="${HOME}/.cache/aro/sdk/ndk/android-ndk-r30-beta3"
MESA_SRC="${MESA_SRC:-/tmp/mesa}"
MESA_BUILD="${MESA_BUILD:-/tmp/mesa-build}"
MESA_TOOLS="${MESA_TOOLS:-/tmp/mesa-tools}"

if [ ! -d "${MESA_SRC}" ]; then
    echo "Cloning Mesa..."
    git clone --depth 1 https://gitlab.freedesktop.org/mesa/mesa.git "${MESA_SRC}"
fi

export PATH="${MESA_TOOLS}/bin:${PATH}"
export PKG_CONFIG_PATH="${HOME}/.local/usr/lib/pkgconfig:${PKG_CONFIG_PATH:-}"
export LD_LIBRARY_PATH="${HOME}/.local/usr/lib:${LD_LIBRARY_PATH:-}"

echo "Configuring Mesa for Android x86_64..."
rm -rf "${MESA_BUILD}"
meson setup "${MESA_BUILD}" "${MESA_SRC}" \
  --cross-file /tmp/mesa-android-x86_64.ini \
  -Dbuildtype=release \
  -Dplatforms=android \
  -Dplatform-sdk-version=37 \
  -Dandroid-stub=true \
  -Dgallium-drivers= \
  -Dvulkan-drivers=intel \
  -Dmesa-clc=system \
  -Dllvm=disabled \
  -Degl=disabled \
  -Dgles1=disabled \
  -Dgles2=disabled \
  -Dgbm=disabled \
  -Dglx=disabled \
  -Dcpp_rtti=false \
  -Dallow-fallback-for=libdrm

echo "Building..."
ninja -C "${MESA_BUILD}"

echo "Installing vendor libraries to ${REPO_DIR}/vendor/lib64..."
mkdir -p "${REPO_DIR}/vendor/lib64/hw"
cp "${MESA_BUILD}/src/intel/vulkan/libvulkan_intel.so" "${REPO_DIR}/vendor/lib64/hw/vulkan.aro.so"
cp "${MESA_BUILD}/subprojects/libdrm-2.4.133/libdrm.so" "${REPO_DIR}/vendor/lib64/libdrm.so"
cp "${MESA_BUILD}/subprojects/expat-2.5.0/libexpat.so" "${REPO_DIR}/vendor/lib64/libexpat.so"
cp "${NDK_DIR}/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/x86_64-linux-android/libc++_shared.so" "${REPO_DIR}/vendor/lib64/libc++_shared.so"
if [ -f "${HOME}/.local/share/aro/system/system/lib64/libz.so" ]; then
    cp "${HOME}/.local/share/aro/system/system/lib64/libz.so" "${REPO_DIR}/vendor/lib64/libz.so"
fi

echo "Mesa Vulkan build complete."
