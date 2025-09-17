#ifndef __VMLINUX_MIN_H__
#define __VMLINUX_MIN_H__

/*
 * Minimal vmlinux.h with only the kernel structures needed for our eBPF socket tracker.
 * This replaces the full vmlinux.h (3.4MB) with just the essential definitions.
 *
 * Generated for Linux kernel structures used in socket tracking.
 * See EBPF_BUILD.md for instructions on how to regenerate or customize.
 */

/* Define this to trigger kernel field names in bpf_tracing.h */
#define __VMLINUX_H__

#ifndef BPF_NO_PRESERVE_ACCESS_INDEX
#pragma clang attribute push (__attribute__((preserve_access_index)), apply_to = record)
#endif

/* Basic kernel types */
typedef unsigned char __u8;
typedef unsigned short __u16;
typedef unsigned int __u32;
typedef unsigned long long __u64;
typedef signed char __s8;
typedef signed short __s16;
typedef signed int __s32;
typedef signed long long __s64;
typedef __u16 __be16;
typedef __u32 __be32;
typedef __u64 __be64;
typedef __u32 __wsum;
typedef __u16 __kernel_sa_family_t;
typedef _Bool bool;

/* Additional type definitions needed for kernel structures */
typedef __u64 __addrpair;
typedef __u32 __portpair;
typedef struct { __s64 counter; } atomic64_t;
typedef struct { __s32 counter; } atomic_t;

/* BPF map types and flags */
enum {
    BPF_MAP_TYPE_HASH = 1,
};

enum {
    BPF_ANY = 0,
};

/* Network address structures */
struct in_addr {
    __be32 s_addr;
};

struct in6_addr {
    union {
        __u8 u6_addr8[16];
        __be16 u6_addr16[8];
        __be32 u6_addr32[4];
    } in6_u;
};

/* Socket address structures */
struct sockaddr_in {
    __kernel_sa_family_t sin_family;
    __be16 sin_port;
    struct in_addr sin_addr;
    unsigned char __pad[8];
};

/* Forward declarations for complex types we don't need to fully define */
struct proto;
struct sk_buff_head;

/* Simple structures we need defined */
struct hlist_node {
    struct hlist_node *next, **pprev;
};

struct iov_iter {
    /* We don't access fields, just need size/layout for msghdr */
    void *__opaque[8]; /* Approximate size, CO-RE will handle differences */
};

/* Minimal possible_net_t - we don't access its internals */
typedef struct {
    void *net; /* We don't dereference this, just need the field present */
} possible_net_t;

/* Socket common structure - core networking fields */
struct sock_common {
    /* Address pair for IPv4 */
    union {
        __addrpair skc_addrpair; /* We don't use this directly */
        struct {
            __be32 skc_daddr;      /* destination IPv4 address */
            __be32 skc_rcv_saddr;  /* source IPv4 address */
        };
    };

    /* Hash - we don't use this but it's part of the layout */
    union {
        unsigned int skc_hash;
        __u16 skc_u16hashes[2];
    };

    /* Port pair */
    union {
        __portpair skc_portpair; /* We don't use this directly */
        struct {
            __be16 skc_dport;   /* destination port */
            __u16 skc_num;      /* source port */
        };
    };

    /* Basic socket properties */
    short unsigned int skc_family;
    volatile unsigned char skc_state;
    unsigned char skc_reuse: 4;
    unsigned char skc_reuseport: 1;
    unsigned char skc_ipv6only: 1;
    unsigned char skc_net_refcnt: 1;
    int skc_bound_dev_if;

    /* Hash table linkage - we don't use these but they're part of layout */
    union {
        struct hlist_node skc_bind_node;
        struct hlist_node skc_portaddr_node;
    };

    /* Protocol and network namespace */
    struct proto *skc_prot;
    possible_net_t skc_net;

    /* IPv6 addresses - these come after the above fields */
    struct in6_addr skc_v6_daddr;
    struct in6_addr skc_v6_rcv_saddr;

    /* Socket cookie for identification */
    atomic64_t skc_cookie;

    /* Additional fields exist but we don't need them for CO-RE access */
};

/* Main socket structure - we only need the common part */
struct sock {
    struct sock_common __sk_common;
    /*
     * Many more fields exist here, but we only access __sk_common
     * CO-RE will handle the field relocations regardless of what
     * other fields are present in different kernel versions
     */
};

/* Message header for sendmsg syscalls */
struct msghdr {
    void *msg_name;           /* Socket name (sockaddr_in* for UDP) */
    int msg_namelen;          /* Length of socket name */
    int msg_inq;              /* Bytes in receive queue */
    struct iov_iter msg_iter; /* Data payload iterator */

    /* Control messages - we don't use these but they're part of layout */
    union {
        void *msg_control;
        void *msg_control_user;
    };
    bool msg_control_is_user: 1;
    bool msg_get_inq: 1;
    /* Additional fields may exist but we only need msg_name */
};

/* FRED (Flexible Return and Event Delivery) support structures */
struct fred_cs {
    __u64 cs: 16;
    __u64 sl: 2;
    __u64 wfe: 1;
};

struct fred_ss {
    __u64 ss: 16;
    __u64 sti: 1;
    __u64 swevent: 1;
    __u64 nmi: 1;
    int: 13;
    __u64 vector: 8;
    short: 8;
    __u64 type: 4;
    char: 4;
    __u64 enclave: 1;
};

/*
 * Architecture-specific pt_regs for kprobe context
 * x86_64 specific - for PT_REGS_PARM1/PT_REGS_PARM2 macros
 * Field names must match kernel exactly for CO-RE relocations
 */
struct pt_regs {
    /*
     * C ABI says these regs are callee-preserved. They aren't saved on kernel entry
     * unless syscall needs a complete, fully filled "struct pt_regs".
     */
    long unsigned int r15;
    long unsigned int r14;
    long unsigned int r13;
    long unsigned int r12;
    long unsigned int bp;    /* Must match kernel BTF field names */
    long unsigned int bx;    /* Must match kernel BTF field names */
    /* These regs are callee-clobbered. Always saved on kernel entry. */
    long unsigned int r11;
    long unsigned int r10;
    long unsigned int r9;
    long unsigned int r8;
    long unsigned int ax;    /* Must match kernel BTF field names */
    long unsigned int cx;    /* Must match kernel BTF field names */
    long unsigned int dx;    /* Must match kernel BTF field names */
    long unsigned int si;    /* Must match kernel BTF field names */
    long unsigned int di;    /* Must match kernel BTF field names */
    /*
     * On syscall entry, this is syscall#. On CPU exception, this is error code.
     * On hw interrupt, it's IRQ number:
     */
    long unsigned int orig_ax;
    /* Return frame for iretq */
    long unsigned int ip;    /* Must match kernel BTF field names */
    union {
        __u16 cs;
        __u64 csx;
        struct fred_cs fred_cs;
    };
    long unsigned int flags; /* Must match kernel BTF field names */
    long unsigned int sp;    /* Must match kernel BTF field names */
    union {
        __u16 ss;
        __u64 ssx;
        struct fred_ss fred_ss;
    };
};

#ifndef BPF_NO_PRESERVE_ACCESS_INDEX
#pragma clang attribute pop
#endif

#endif /* __VMLINUX_MIN_H__ */