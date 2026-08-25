/* Novos-OS 内核链接脚本（x86-64，multiboot2，加载基址 1MB） */
ENTRY(_start)

SECTIONS {
    . = 1M;

    /* multiboot2 头必须在文件前 32768 字节内，8 字节对齐 */
    .multiboot2 : ALIGN(8) {
        KEEP(*(.multiboot2))
    }

    .text : ALIGN(4K) {
        *(.text .text.*)
    }

    .rodata : ALIGN(4K) {
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
        *(COMMON)
    }
}
