/* 山水观心操作系统内核链接脚本（x86-64，multiboot2 + PVH，加载基址 1MB） */
ENTRY(_start)

PHDRS {
    text PT_LOAD;
    note PT_NOTE;
}

SECTIONS {
    . = 1M;

    /* multiboot1 头（QEMU 扁平镜像）必须在文件最前 */
    .multiboot1 : ALIGN(4) {
        KEEP(*(.multiboot1))
    } :text

    /* multiboot2 头必须在文件前 32768 字节内，8 字节对齐 */
    .multiboot2 : ALIGN(8) {
        KEEP(*(.multiboot2))
    } :text

    /* PVH note（QEMU -kernel 用，Xen PHYS32_ENTRY）——须在 PT_NOTE 段 */
    .note.Xen : ALIGN(4) {
        KEEP(*(.note.Xen))
    } :note :text

    .text : ALIGN(4K) {
        *(.text .text.*)
    } :text

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    } :text

    .data : ALIGN(4K) {
        *(.data .data.*)
    } :text

    /* 注意：multiboot1 头的 load_end_addr 设为 0（加载到文件末尾），
       故这里不再需要 _kernel_load_end 符号——.bss 之后的孤儿段
       （如 .got）不会造成 QEMU 读文件越界。 */

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
        *(COMMON)
    } :text

    /* 内核镜像末尾符号（multiboot1 头的 bss_end 字段用） */
    _kernel_end = .;
}
