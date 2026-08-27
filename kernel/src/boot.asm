# 山水观心操作系统启动汇编（x86-64）
# DESIGN.md §1.3 Phase 1：设栈 → 读取 multiboot2 参数 → 建直接映射页表（2MB 大页）→ 进入长模式
# 同时包含异常 stub 表（供 interrupts.rs 填充 IDT）。

.set MB2_MAGIC,       0xE85250D6     # multiboot2 头魔数
.set MB2_ARCH_I386,   0              # i386 protected mode
.set MB2_BOOT_MAGIC,  0x36D76289     # bootloader 回传的 magic（eax）

# ---------------------------------------------------------------------------
# multiboot1 头（QEMU 对 ELF 走 PVH、对扁平镜像走 multiboot loader；
# 本头用于 objcopy 扁平镜像经 QEMU -kernel 启动；GRUB 走 multiboot2 头）
# flags: MEMINFO(0x1) | ALIGN(0x2) | LOAD_ADDR(0x10000)
# ---------------------------------------------------------------------------
.section .multiboot1, "a"
.align 4
multiboot1_start:
    .long 0x1BADB002
    .long 0x00010003
    .long -(0x1BADB002 + 0x00010003)
    .long 0x100000            # header_addr（头自身物理地址）
    .long 0x100000            # load_addr
    .long 0                   # load_end_addr（0 = 加载到文件末尾；QEMU/GRUB 均支持）
    .long _kernel_end         # bss_end_addr（QEMU 清零到此处）
    .long _start              # entry_addr
multiboot1_end:

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
# PVH ELF note（QEMU -kernel 走 PVH 路径；GRUB 走上面的 multiboot2 路径）
# 独立文件 boot_note.asm：仅真实内核构建（非 test）引入——host 测试汇编器
# 不识别 `.note.Xen, "a", @note` 的 GNU 语法（见 lib.rs 的 cfg 注释）。
# ---------------------------------------------------------------------------

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
.global pdpt
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
# 长模式 GDT（null / kernel code64 / kernel data / user data / user code64 / TSS）
# 选择子：0x08=kcode, 0x10=kdata, 0x18=udata, 0x20=ucode, 0x28=TSS
# syscall/sysret 段规则（STAR MSR）：
#   SYSCALL 进入：CS=STAR[47:32](0x08), SS=STAR[47:32]+8(0x10)
#   SYSRET 回 ring3：CS=STAR[63:48]+16(0x20), SS=STAR[63:48]+8(0x18)
#   故 STAR = (0x10 << 48) | (0x08 << 32)
# ---------------------------------------------------------------------------
.section .rodata
.align 8
.global gdt64
gdt64:
    .quad 0x0000000000000000        # null
    .quad 0x0020980000000000        # 0x08 kernel code: DPL0, L=1
    .quad 0x0000920000000000        # 0x10 kernel data: DPL0
    .quad 0x0000F20000000000        # 0x18 user data: DPL3
    .quad 0x0020FA0000000000        # 0x20 user code: DPL3, L=1
    # TSS 描述符（0x28，16 字节，type=0x89 64-bit TSS available）
    # base 字段由 gdt.rs 运行时填充（.word tss 会触发 R_X86_64_16 重定位超范围）
    .word (tss_end - tss - 1)       # limit[15:0] = 103
    .word 0                         # base[15:0] —— gdt.rs 填充
    .byte 0                         # base[23:16] —— gdt.rs 填充
    .byte 0x89                      # P=1, DPL=0, type=0b1001 (64-bit TSS)
    .byte 0x00                      # flags + limit[19:16]
    .byte 0                         # base[31:24] —— gdt.rs 填充
    .long 0                         # base[63:32] —— gdt.rs 填充
    .long 0                         # reserved
gdt64_ptr:
    .word gdt64_ptr - gdt64 - 1
    .long gdt64           # 32-bit lgdt 用 6 字节 GDTR（gdt64 地址 < 4GB）

# ---------------------------------------------------------------------------
# TSS（64-bit，104 字节；syscall/中断从用户态切回内核时用 RSP0 作内核栈）
# ---------------------------------------------------------------------------
.section .data
.align 16
.global tss
tss:
    .long 0          # reserved (offset 0)
tss_rsp0:
    .quad 0          # RSP0 (offset 4) —— 由 gdt.rs 设置
    .quad 0          # RSP1 (12)
    .quad 0          # RSP2 (20)
    .quad 0          # reserved (28)
    .quad 0          # IST1 (36)
    .quad 0          # IST2 (44)
    .quad 0          # IST3 (52)
    .quad 0          # IST4 (60)
    .quad 0          # IST5 (68)
    .quad 0          # IST6 (76)
    .quad 0          # IST7 (84)
    .quad 0          # reserved (92)
    .word 0          # reserved (100)
    .word 0          # IOPB (102)
tss_end:

# ---------------------------------------------------------------------------
# syscall 专用内核栈（用户态 syscall 进入时切换到此处）
# ---------------------------------------------------------------------------
.section .bss
.align 16
syscall_stack_bottom:
    .skip 16 * 1024
syscall_stack_top:

# ---------------------------------------------------------------------------
# syscall 入口（由 syscall 指令进入）：
# 进入时 RCX=用户 RIP, R11=用户 RFLAGS, RSP=用户栈, CS=0x08, SS=0x10（CPU 不切栈）。
# 构造与中断一致的 ExceptionFrame，iretq 返回（复用 Rust handler 模式）。
#
# 注意：syscall 须透传全部通用寄存器（仅 rcx/r11 例外）。本入口需用 r12 暂存
# 用户 RSP，故先把用户 r12 溢出到用户栈 [rsp-8]（ABI 红区/scratch；iretq 恢复
# 原始 rsp 后该位置对用户不可见），再在 rust_syscall_handler 中取回修复。
# ---------------------------------------------------------------------------
.section .text
.global syscall_entry
syscall_entry:
    pushq %r12             # [用户栈 rsp-8] = 用户 r12（溢出保存）
    movq %rsp, %r12        # r12 = 用户 rsp-8
    # M6-切片1：内核栈取当前任务（TSS.RSP0，随任务切换更新；task 0 = syscall 栈）。
    # 注意：必须用 RIP 相对寻址——直接 movabs 会冲掉用户 rax（syscall 号）。
    movq tss_rsp0(%rip), %rsp
    # 构造 ExceptionFrame（从高到低 push：ss,rsp,rflags,cs,rip,err,vec,rax..r15）
    pushq $0x1B            # ss = user data (0x18 | 3)
    pushq %r12             # rsp（= 用户 rsp-8，rust_syscall_handler 修复为原值）
    pushq %r11             # rflags
    pushq $0x23            # cs = user code (0x20 | 3)
    pushq %rcx             # rip = 用户返回地址
    pushq $0               # err
    pushq $0               # vec（syscall 伪向量）
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
    pushq %r12             # r12（= 用户 rsp-8，rust_syscall_handler 修复为用户 r12）
    pushq %r13
    pushq %r14
    pushq %r15
    movq %rsp, %rdi        # frame 指针
    call rust_syscall_handler   # 返回目标任务 frame（fork 时为子帧，普通 syscall 为原帧）
    movq %rax, %rsp        # M6-切片1：切到返回帧（fork 子先执行；普通 syscall 等效原帧）
    # 恢复通用寄存器
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp         # 跳过 vec + err
    iretq                  # 弹 rip/cs/rflags/rsp/ss 回用户态（rsp 为修复后的原值）

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
# fork 包装：捕获 fork 点 callee-saved 寄存器（供子任务恢复局部变量），
# 并把返回地址、保存寄存器指针、目标 rsp 传给 rust_fork_impl。
# 父路径：call 返回后弹出保存的寄存器并 ret（cid 在 eax）。
# 子路径：从 ret_addr 恢复，rsp=目标 rsp，rax=0，寄存器=fork 点快照。
# ---------------------------------------------------------------------------
.global fork_wrapper
fork_wrapper:
    pushq %r15
    pushq %r14
    pushq %r13
    pushq %r12
    pushq %rbx
    pushq %rbp
    movq 48(%rsp), %rdi        # 返回地址（worker 的 fork 调用点）
    movq %rsp, %rsi            # 保存的寄存器块指针（r15..rbp 顺序）
    leaq 56(%rsp), %rdx        # worker 恢复后应有的 rsp
    call rust_fork_impl        # rax = 子任务 id（父侧）
    popq %rbp
    popq %rbx
    popq %r12
    popq %r13
    popq %r14
    popq %r15
    ret

# ---------------------------------------------------------------------------
# 外设中断 stub（PIC 重映射后向量 32-47）：压 err=0 + vec，跳 irq_common。
# 与异常 stub 同布局（每条 9 字节，16 字节对齐），供 interrupts.rs 计算地址。
# ---------------------------------------------------------------------------
.macro IRQ_ENTRY vec
.align 16
irq_stub_\vec:
    pushq $0
    pushq $\vec
    jmp irq_common
.endm

IRQ_ENTRY 32
IRQ_ENTRY 33
IRQ_ENTRY 34
IRQ_ENTRY 35
IRQ_ENTRY 36
IRQ_ENTRY 37
IRQ_ENTRY 38
IRQ_ENTRY 39
IRQ_ENTRY 40
IRQ_ENTRY 41
IRQ_ENTRY 42
IRQ_ENTRY 43
IRQ_ENTRY 44
IRQ_ENTRY 45
IRQ_ENTRY 46
IRQ_ENTRY 47

.global irq_stub_base
irq_stub_base = irq_stub_32

# ---------------------------------------------------------------------------
# 公共中断入口：保存 15 个通用寄存器 → rust_irq_handler（返回目标任务的
# ExceptionFrame 指针，用于任务切换）→ 恢复新栈 → iretq。
# ---------------------------------------------------------------------------
irq_common:
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
    call rust_irq_handler        # rax = 目标任务 frame 指针
    movq %rax, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp               # 跳过 vec + err
    iretq

# ---------------------------------------------------------------------------
# 公共异常入口：保存 15 个通用寄存器 → rust_exception_handler
# （返回目标任务 frame 指针——用户态可恢复异常如 #PF→SIGSEGV 经 iretq 回用户；
#   不可恢复异常内部停机不返回）→ 恢复新栈 → iretq。
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
    call rust_exception_handler    # rax = 目标 frame 指针（panic 路径不返回）
    movq %rax, %rsp
    popq %r15
    popq %r14
    popq %r13
    popq %r12
    popq %r11
    popq %r10
    popq %r9
    popq %r8
    popq %rbp
    popq %rdi
    popq %rsi
    popq %rdx
    popq %rcx
    popq %rbx
    popq %rax
    addq $16, %rsp               # 跳过 vec + err
    iretq
