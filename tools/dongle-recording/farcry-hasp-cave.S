# Far Cry Paradise Lost (FarCry_r.exe, MD5 87648806d0b4b5a5384e7eedf4882d7e): logs every HASP4 hasp() call.
# Placed in the free tail of .text (0x8f96b0); the four `call hasp` sites are repointed here.
# hasp(service, seed, lpt, pw1, pw2, &p1, &p2, &p3, &p4), cdecl. The real hasp() is called
# first (so whatever answers it, any dongle emulation included, still does), then a record
# is appended to hasplog.bin (opened once, in the current directory):
#   "HSP4" | 9 arguments | p1 p2 p3 p4 after the call | ReadBlock (50): 112 bytes at p4
    .set HASP,        0x8a9094
    .set CREATEFILEA, 0x8fa0f8      # import slots
    .set WRITEFILE,   0x8fa148
    .set HANDLE,      0xa52f20      # free tail of .data (after its virtual size, zeroed)
    .set WRITTEN,     0xa52f24
    .set REC,         0xa52f28      # record buffer, 4 + 13*4 + 112 = 168 bytes

    .text
    .globl _start
_start:
    pusha
    lea   0x24(%esp), %esi          # arguments (after pusha 0x20 + return address)
    mov   $9, %ecx
1:  pushl -4(%esi,%ecx,4)
    loop  1b
    call  HASP
    add   $0x24, %esp
    mov   %eax, 0x1c(%esp)          # keep hasp's eax across popa

    mov   $REC, %edi
    movl  $0x34505348, (%edi)       # "HSP4"
    add   $4, %edi
    mov   $9, %ecx
    rep movsl                       # the 9 arguments (esi -> args)
    lea   0x24(%esp), %esi
    mov   $5, %ebx                  # &p1..&p4 are arguments 6..9
2:  mov   (%esi,%ebx,4), %eax
    mov   (%eax), %eax
    stosl
    inc   %ebx
    cmp   $9, %ebx
    jb    2b
    mov   $(4 + 13*4), %edx
    cmpl  $50, (%esi)               # ReadBlock: dump the memory read
    jne   3f
    mov   0x20(%esi), %eax          # &p4
    mov   (%eax), %esi              # p4 = buffer
    mov   $28, %ecx
    rep movsl
    add   $112, %edx
3:
    mov   HANDLE, %eax
    test  %eax, %eax
    jnz   4f
    push  %edx
    push  $0                        # template
    push  $0x80                     # FILE_ATTRIBUTE_NORMAL
    push  $2                        # CREATE_ALWAYS
    push  $0
    push  $1                        # FILE_SHARE_READ
    push  $0x40000000               # GENERIC_WRITE
    call  5f
    .asciz "hasplog.bin"
5:  call  *CREATEFILEA
    mov   %eax, HANDLE
    pop   %edx
4:  push  $0
    push  $WRITTEN
    push  %edx
    push  $REC
    push  HANDLE
    call  *WRITEFILE
    popa
    ret
