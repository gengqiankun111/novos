# Novos-OS 启动汇编（x86-64）
# DESIGN.md §1.3 Phase 1：设栈 → 读取 multiboot2 参数 → 建直接映射页表（2MB 大页）→ 进入长模式
# 同时包含异常 stub 表（供 interrupts.rs 填充 IDT）。

.set MB2_MAGIC,       0xE85250D6     # multiboot2 头魔数
.set MB2_ARCH_I386,   0              # i386 protected mode
.set MB2_BOOT_MAGIC,  0x36D76289     # bootloader 回传的 magic（eax）

# ---------------------------------------------------------------------------
# multiboot2 头（16 字节 + end tag）
# ---------------------------------------------------------------------------
.section .multiboot2, "a"
.align 8
header_start:
    .long MB2_MAGIC
    .long MB2_ARCH_I386
    .long header_end - header_start
    .long -(MB2_MAGIC + MB2_ARCH_I386 + (header_end - header_start))
    .short 0            # tag: end
    .short 0
    .long 8
header_end:

# ---------------------------------------------------------------------------
# BSS：内核栈；DATA：页表（1GB 恒等映射，含重定位，须放 .data）
# ---------------------------------------------------------------------------
.section .bss, "aw"
.align 16
boot_stack_bottom:
    .skip 64 * 1024
boot_stack_top:

.section .data, "aw"
.align 4096
pml4:
    .quad pdpt + 0x03
    .skip 4096 - 8
pdpt:
    .quad pd + 0x03
    .skip 4096 - 8
pd:
    .set i, 0
    .rept 512
    .quad (i * 0x200000 + 0x83)     # 2MB 大页：present|write|PS
    .set i, i + 1
    .endr

# ---------------------------------------------------------------------------
# 入口：_start（32 位保护模式，分页关闭）
# ---------------------------------------------------------------------------
.section .text
.global _start
.code32
_start:
    cli
    # 保存 bootloader 参数：eax=magic → rdi(参数0)，ebx=mbi 地址 → rsi(参数1)
    movl %eax, %edi
    movl %ebx, %esi
    # 设栈（恒等映射 1GB 覆盖 .bss）
    movl $boot_stack_top, %esp
    # 开启 PAE
    movl %cr4, %eax
    orl  $0x20, %eax
    movl %eax, %cr4
    # 加载页表
    movl $pml4, %eax
    movl %eax, %cr3
    # EFER.LME = 1
    movl $0xC0000080, %ecx
    rdmsr
    orl  $0x100, %eax
    wrmsr
    # CR0.PG + CR0.PE
    movl %cr0, %eax
    orl  $0x80000001, %eax
    movl %eax, %cr0
    # 加载 64 位 GDT，远跳到长模式
    lgdt gdt64_ptr
    ljmp $0x08, $long_mode_start

.code64
long_mode_start:
    # 设置数据段
    movw $0x10, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %fs
    movw %ax, %gs
    movw %ax, %ss
    # 开启 SSE：CR0.MP + CR4.OSFXSR|OSXMMEXCPT
    movq %cr0, %rax
    orq  $0x2, %rax
    movq %rax, %cr0
    movq %cr4, %rax
    orq  $0x600, %rax
    movq %rax, %cr4
    # 栈 16 字节对齐（System V ABI）
    andq $-16, %rsp
    # 进入 Rust 入口（rdi=magic, rsi=mbi_addr）
    call rust_start
1:
    hlt
    jmp 1b

# ---------------------------------------------------------------------------
# 长模式 GDT（null / code64 / data）
# ---------------------------------------------------------------------------
.section .rodata
gdt64:
    .quad 0x0000000000000000        # null
    .quad 0x0020980000000000        # code: DPL0, L=1
    .quad 0x0000920000000000        # data: DPL0
gdt64_ptr:
    .word gdt64_ptr - gdt64 - 1
    .long gdt64

# ---------------------------------------------------------------------------
# 异常 stub 表：每个向量一个 16 字节槽，统一进入 exception_common
# 栈布局（stub 后、通用寄存器保存前，顶→下）：[vec, err, rip, cs, rflags, rsp, ss]
#   - 无错误码向量：stub 先压 0 作为 err
#   - 有错误码向量（8,10-14,17）：CPU 已压 err，stub 只压 vec
# ---------------------------------------------------------------------------
.macro EXC_NOERR vec
.align 16
stub_\vec:
    pushq $0
    pushq $\vec
    jmp exception_common
.endm

.macro EXC_ERR vec
.align 16
stub_\vec:
    pushq $\vec
    jmp exception_common
.endm

EXC_NOERR 0    # #DE
EXC_NOERR 1    # #DB
EXC_NOERR 2    # NMI
EXC_NOERR 3    # #BP
EXC_NOERR 4    # #OF
EXC_NOERR 5    # #BR
EXC_NOERR 6    # #UD
EXC_NOERR 7    # #NM
EXC_ERR   8    # #DF
EXC_NOERR 9    # #MF
EXC_ERR   10   # #TS
EXC_ERR   11   # #NP
EXC_ERR   12   # #SS
EXC_ERR   13   # #GP
EXC_ERR   14   # #PF
EXC_NOERR 15   # (reserved)
EXC_NOERR 16   # #MF
EXC_ERR   17   # #AC
EXC_NOERR 18   # #MC
EXC_NOERR 19   # #XM
EXC_NOERR 20   # #VE
EXC_NOERR 21   # #CP
EXC_NOERR 22
EXC_NOERR 23
EXC_NOERR 24
EXC_NOERR 25
EXC_NOERR 26
EXC_NOERR 27
EXC_NOERR 28
EXC_NOERR 29
EXC_NOERR 30
EXC_NOERR 31

.global stub_base
stub_base = stub_0

# ---------------------------------------------------------------------------
# 公共异常入口：保存 15 个通用寄存器 → 调用 rust_exception_handler（不返回）
# ---------------------------------------------------------------------------
exception_common:
    pushq %rax
    pushq %rbx
    pushq %rcx
    pushq %rdx
    pushq %rsi
    pushq %rdi
    pushq %rbp
    pushq %r8
    pushq %r9
    pushq %r10
    pushq %r11
    pushq %r12
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi
    call rust_exception_handler
1:
    hlt
    jmp 1b
