# IYAKKI legacy BIOS boot sector for disks with 4096-byte sectors (F-INST-29, D-4KN-BIOS).
#
# 16-bit real mode, 440 bytes of boot code. The BIOS of a 4Kn disk loads the whole 4096-byte sector 0 at 0x7C00; only the first 512
# bytes matter here. GRUB's stock boot.img counts in 512-byte sectors and would not start, so this code does what boot.img and the
# first sector of GRUB's core image (diskboot) do together, in 4096-byte units:
#   1. read the core image's first sector (4096 bytes, its address in the 8-byte field at 0x5C, written by the installer) to 0x8000;
#   2. read the core image's block list entry at 0x8000+500 (start, length in 4096-byte sectors, load segment), at most 8 sectors per
#      call, stepping the load segment by 0x100 paragraphs (4096 bytes) per sector read;
#   3. jump to 0x0000:0x9000 (just behind the first sector, which is now 4096 bytes).
# An error prints a short message and halts. The assembler is LLVM's integrated one (Apache-2.0 with LLVM exception); the checked-in
# boot4k.bin is its output (build.sh), and a test runs that file on a small real-mode interpreter against a fake BIOS.
# The code ends with the marker that iyakki_usb::bios4kn::MARKER names: that is what says "I read 4096-byte sectors".
    .code16
    .text
    .globl _start
_start:
    jmp start
    .org 0x54
dap:                            # EDD disk address packet; its LBA field is the installer's field at 0x5C
    .byte 0x10, 0
    .word 1                     # sectors
    .word 0                     # buffer offset
    .word 0x0800                # buffer segment (0x8000)
dap_lba:
    .quad 0                     # 0x5C: the core image's first sector, in 4096-byte units (written by the installer)
    .org 0x64
start:
    cli
    xorw %ax, %ax
    movw %ax, %ds
    movw %ax, %es
    movw %ax, %ss
    movw $0x7C00, %sp
    sti
    ljmp $0, $go
go:
    movb %dl, drive
    movb $0x41, %ah             # EDD present?
    movw $0x55AA, %bx
    int $0x13
    jc fail
    cmpw $0xAA55, %bx
    jne fail
    testb $1, %cl
    jz fail
    call readone                # the core image's first sector to 0x8000
    movw 0x81F4+8, %ax          # block list entry: start (8), sectors (2), segment (2)
    movw %ax, left
    movw 0x81F4+10, %ax
    movw %ax, dap+6             # buffer segment
    movw $0, dap+4
    movw 0x81F4, %ax            # its start sector replaces the field
    movw %ax, dap+8
    movw 0x81F4+2, %ax
    movw %ax, dap+10
    movw 0x81F4+4, %ax
    movw %ax, dap+12
    movw 0x81F4+6, %ax
    movw %ax, dap+14
more:
    movw left, %ax
    testw %ax, %ax
    jz run
    movw dap+6, %cx             # at most up to the next 64 KiB boundary (a BIOS may refuse a read across it), and at most 8 sectors
    movb %ch, %cl
    andw $15, %cx
    negw %cx
    addw $16, %cx
    cmpw $8, %cx
    jbe near
    movw $8, %cx
near:
    cmpw %cx, %ax
    jbe few
    movw %cx, %ax
few:
    movw %ax, dap+2
    call readone
    movw dap+2, %ax
    subw %ax, left
    addw %ax, dap+8             # next start sector, with the carry through all 8 bytes
    adcw $0, dap+10
    adcw $0, dap+12
    adcw $0, dap+14
    movb %al, %ah               # segment += sectors * 0x100
    xorb %al, %al
    addw %ax, dap+6
    jmp more
run:
    movb drive, %dl
    ljmp $0, $0x9000
readone:
    movw $dap, %si
    movb drive, %dl
    movb $0x42, %ah
    int $0x13
    jc fail
    ret
fail:
    movw $msg, %si
1:  lodsb
    testb %al, %al
    jz 2f
    movb $0x0E, %ah
    movw $7, %bx
    int $0x10
    jmp 1b
2:  hlt
    jmp 2b
msg:
    .asciz "IYAKKI 4K boot error\r\n"
    .org 420
drive:
    .byte 0, 0
left:
    .word 0
    .ascii "IYAKKI-4K-BOOT\001\0"   # marker, bytes 424..440
    .org 440
