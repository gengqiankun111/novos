# Novos-OS 构建/运行（M0）
#
# 说明：内核为自包含 multiboot2 ELF，QEMU 可直接 -kernel 启动；
# 无需 bootloader 镜像构建器（避免宿主链接依赖）。

KERNEL_TARGET := x86_64-unknown-none
KERNEL_ELF    := target/$(KERNEL_TARGET)/release/novos-kernel

.PHONY: build run qemu clean

build:
	cargo build -p novos-kernel --target $(KERNEL_TARGET) --release

run: qemu

qemu: build
	qemu-system-x86_64 \
		-kernel $(KERNEL_ELF) \
		-m 64M \
		-serial stdio \
		-nographic \
		-no-reboot \
		-display none

clean:
	cargo clean
