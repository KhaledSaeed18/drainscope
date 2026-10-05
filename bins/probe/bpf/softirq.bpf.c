// SPDX-License-Identifier: GPL-3.0-or-later
//
// Measures the CPU time the kernel spends in the network softirqs (NET_TX and NET_RX): the
// network stack's work on behalf of every socket, which cgroup accounting charges to no app.
// Only two machine-wide totals leave the kernel (ADR 0007).

#include <stdbool.h>
#include <linux/types.h>
#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_tracing.h>

#define NET_TX_SOFTIRQ 2
#define NET_RX_SOFTIRQ 3

// When the current network softirq started on this CPU; 0 when none is running. Softirqs
// don't nest on one CPU.
struct {
	__uint(type, BPF_MAP_TYPE_PERCPU_ARRAY);
	__uint(max_entries, 1);
	__type(key, __u32);
	__type(value, __u64);
} started SEC(".maps");

// Cumulative nanoseconds: index 0 for NET_TX, 1 for NET_RX.
struct {
	__uint(type, BPF_MAP_TYPE_ARRAY);
	__uint(max_entries, 2);
	__type(key, __u32);
	__type(value, __u64);
} network_ns SEC(".maps");

static __always_inline bool is_network(unsigned int vec)
{
	return vec == NET_TX_SOFTIRQ || vec == NET_RX_SOFTIRQ;
}

SEC("tp_btf/softirq_entry")
int BPF_PROG(network_softirq_entry, unsigned int vec)
{
	if (!is_network(vec))
		return 0;
	__u32 zero = 0;
	__u64 *start = bpf_map_lookup_elem(&started, &zero);
	if (start)
		*start = bpf_ktime_get_ns();
	return 0;
}

SEC("tp_btf/softirq_exit")
int BPF_PROG(network_softirq_exit, unsigned int vec)
{
	if (!is_network(vec))
		return 0;
	__u32 zero = 0;
	__u64 *start = bpf_map_lookup_elem(&started, &zero);
	// Attached mid-softirq: no start to measure from.
	if (!start || *start == 0)
		return 0;
	__u64 elapsed = bpf_ktime_get_ns() - *start;
	*start = 0;
	__u32 index = vec - NET_TX_SOFTIRQ;
	__u64 *total = bpf_map_lookup_elem(&network_ns, &index);
	if (total)
		__sync_fetch_and_add(total, elapsed);
	return 0;
}

char LICENSE[] SEC("license") = "GPL";
