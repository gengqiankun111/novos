/* Novos-OS 用户态 init/shell 链接脚本（M3 切片4）。
 * 加载地址 0x8000000000（512GB），与内核用户页表布局一致
 * （用户代码 512GB、用户栈 512GB+32MB）。
 * 注意：lld 链接脚本数字不支持下划线，写 0x8000000000（10 位 = 2^39）。 */

ENTRY(_start)

BASE = 0x8000000000;

SECTIONS {
    . = BASE;

    .text : ALIGN(4K) {
        *(.text .text.*)
        *(.rodata .rodata.*)
    }

    .data : ALIGN(4K) {
        *(.data .data.*)
    }

    .bss : ALIGN(4K) {
        *(.bss .bss.*)
        *(COMMON)
    }

    /DISCARD/ : {
        *(.comment)
        *(.eh_frame)
        *(.gcc_except_table)
    }
}
