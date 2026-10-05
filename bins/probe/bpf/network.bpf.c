// SPDX-License-Identifier: GPL-3.0-or-later
//
// Counts network bytes per cgroup: received (ingress) and sent (egress), keyed by the cgroup v2
// ID of the socket's owner. Attached to the root cgroup alongside other programs (multi
// attach), so it sees every socket. Only byte totals leave the kernel: no addresses, ports or
// contents.
//
// These programs must always return 1. A cgroup_skb program that returns 0 drops the packet.

#include <linux/types.h>
#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>

// Loopback (ifindex 1 in every network namespace) is local traffic, not the radio's.
#define LOOPBACK_IFINDEX 1

#define ALLOW 1

struct {
	__uint(type, BPF_MAP_TYPE_LRU_HASH);
	__uint(max_entries, 16384);
	__type(key, __u64);
	__type(value, __u64);
} received SEC(".maps");

struct {
	__uint(type, BPF_MAP_TYPE_LRU_HASH);
	__uint(max_entries, 16384);
	__type(key, __u64);
	__type(value, __u64);
} sent SEC(".maps");

static __always_inline void count(void *map, struct __sk_buff *skb)
{
	if (skb->ifindex == LOOPBACK_IFINDEX)
		return;
	__u64 cgroup = bpf_skb_cgroup_id(skb);
	__u64 bytes = skb->len;
	__u64 *total = bpf_map_lookup_elem(map, &cgroup);
	if (total) {
		__sync_fetch_and_add(total, bytes);
	} else {
		// Another CPU may insert the same key first; losing one packet's bytes is fine.
		bpf_map_update_elem(map, &cgroup, &bytes, BPF_NOEXIST);
	}
}

SEC("cgroup_skb/ingress")
int count_received(struct __sk_buff *skb)
{
	count(&received, skb);
	return ALLOW;
}

SEC("cgroup_skb/egress")
int count_sent(struct __sk_buff *skb)
{
	count(&sent, skb);
	return ALLOW;
}

char LICENSE[] SEC("license") = "GPL";
