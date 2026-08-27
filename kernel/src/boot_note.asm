# 山水观心操作系统 PVH ELF note（QEMU -kernel 走 PVH 路径；GRUB 走 multiboot2 路径）
# XEN_ELFNOTE_PHYS32_ENTRY: QEMU 在 32 位保护模式、分页关闭下跳转 _start，
# ebx = &hvm_start_info（与 multiboot2 的 mbi 位置一致）。
#
# 独立成文件：`.section .note.Xen, "a", @note` 是 GNU/ELF 语法，
# host `cargo test`（COFF 目标）的汇编器不识别，故仅在真实内核构建
# （非 test）时经 `#[cfg(not(test))]` 引入（见 lib.rs）。
.section .note.Xen, "a", @note
    .align 4
    .long 2f - 1f      # namesz
    .long 4f - 3f      # descsz
    .long 6            # type: XEN_ELFNOTE_PHYS32_ENTRY
1:  .asciz "Xen"
2:  .align 4
3:  .quad _start
4:  .align 4
