# 山水观心操作系统构建/运行（M0 已通过 QEMU 验证）
#
# 启动链路：自包含 multiboot1 扁平镜像（objcopy）→ QEMU -kernel。
# 也支持：GRUB multiboot2（.multiboot2 头）、QEMU PVH ELF（.note.Xen）。

KERNEL_TARGET := x86_64-unknown-none
KERNEL_ELF    := target/$(KERNEL_TARGET)/release/shanshui-guanxin-kernel
KERNEL_BIN    := target/shanshui-guanxin-kernel.bin

# rustup 安装的 cargo 路径（便携 make 的 MSYS bash 会丢弃 Windows PATH 中的
# 部分条目；cargo bin 目录需显式补到 PATH。MSYS bash 只认 /c/... 风格路径，
# 故用 cygpath 转换；Linux 下该变量为空、无副作用）
CARGO_DIR := $(shell cygpath -u "$(USERPROFILE)/.cargo/bin" 2>/dev/null)
RUST_PATH := PATH="$(CARGO_DIR):$$PATH"

# objcopy：Windows(MSVC) 宿主用 llvm-objcopy，Linux 用 rust-objcopy
ifeq ($(OS),Windows_NT)
  LLVM_OBJCOPY := $(shell rustc --print sysroot)/lib/rustlib/x86_64-pc-windows-msvc/bin/llvm-objcopy
else
  LLVM_OBJCOPY := $(shell rustc --print sysroot)/lib/rustlib/$(KERNEL_TARGET)/bin/rust-objcopy
endif

# QEMU：Windows 用安装目录完整路径（可用命令行 `make QEMU=...` 覆盖）
ifeq ($(OS),Windows_NT)
  QEMU ?= "C:/Program Files/qemu/qemu-system-x86_64.exe"
else
  QEMU ?= qemu-system-x86_64
endif

.PHONY: build build-userspace image run qemu test test-integration test-memory clean

# 先构建用户态 init/shell（ELF 嵌入内核镜像），再构建内核。
# userspace 的链接参数由 userspace/.cargo/config.toml 提供（CWD=userspace 生效）。
# cargo 不跟踪链接脚本变化：linker.ld 比 main.rs 新时 touch main.rs 强制重链
# （宿主机无 MSVC link.exe，userspace 不能带 build.rs）。
build-userspace:
	@if [ userspace/linker.ld -nt userspace/src/main.rs ]; then \
		echo "linker.ld changed, touching main.rs to force relink"; \
		touch userspace/src/main.rs; \
	fi
	cd userspace && $(RUST_PATH) cargo build --release

# 内核链接参数用 RUSTFLAGS env（根 .cargo/config.toml 已清空，避免泄漏进 userspace）
build: build-userspace
	$(RUST_PATH) RUSTFLAGS="-C link-arg=-Tkernel/linker.ld -C relocation-model=static -C relro-level=off" cargo build -p shanshui-guanxin-kernel --target $(KERNEL_TARGET) --release

# ELF → 扁平二进制（QEMU multiboot loader 需要非 ELF 文件）
image: build
	$(LLVM_OBJCOPY) -O binary $(KERNEL_ELF) $(KERNEL_BIN)

run: qemu

qemu: image
	$(QEMU) \
		-kernel $(KERNEL_BIN) \
		-m 64M \
		-serial stdio \
		-nographic \
		-no-reboot \
		-display none

# 集成测试（Windows 便携 make）：QEMU 引导 -> 用户态 shell -> 注入命令 -> 断言输出。
# 说明：裸机内核无法用 cargo test（host 目标编译 boot.asm 会因 .note.Xen 报错，
# 且 no_std 内核无可运行测试），故测试走 QEMU 真实执行路径。
test: image
	powershell -ExecutionPolicy Bypass -File scripts/test-boot.ps1 -Mode boot
	powershell -ExecutionPolicy Bypass -File scripts/test-boot.ps1 -Mode shell

test-integration: image
	timeout 15 $(QEMU) -kernel $(KERNEL_BIN) -m 64M -serial file:target/integration.log -display none -no-reboot -no-shutdown -d guest_errors || true
	@echo "=== integration log ==="; cat target/integration.log

test-memory: image
	timeout 15 $(QEMU) -kernel $(KERNEL_BIN) -m 64M -serial file:target/memory.log -display none -no-reboot -no-shutdown || true
	@echo "=== memory log ==="; cat target/memory.log

clean:
	cargo clean
	-rm -f $(KERNEL_BIN)
