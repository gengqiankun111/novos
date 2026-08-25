# Novos-OS 构建/运行（M0 已通过 QEMU 验证）
#
# 启动链路：自包含 multiboot1 扁平镜像（objcopy）→ QEMU -kernel。
# 也支持：GRUB multiboot2（.multiboot2 头）、QEMU PVH ELF（.note.Xen）。

KERNEL_TARGET := x86_64-unknown-none
KERNEL_ELF    := target/$(KERNEL_TARGET)/release/novos-kernel
KERNEL_BIN    := target/novos-kernel.bin
LLVM_OBJCOPY  := $(shell rustc --print sysroot)/lib/rustlib/$(KERNEL_TARGET)/bin/rust-objcopy
QEMU          := qemu-system-x86_64

.PHONY: build image run qemu clean

build:
	cargo build -p novos-kernel --target $(KERNEL_TARGET) --release

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

clean:
	cargo clean
	-rm -f $(KERNEL_BIN)
