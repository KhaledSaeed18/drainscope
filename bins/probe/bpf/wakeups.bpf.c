// SPDX-License-Identifier: GPL-3.0-or-later
//
// Counts idle exits per cgroup: every switch from a CPU's idle task to another task, keyed by
// the incoming task's cgroup v2 ID (the cgroup directory's inode number). Only these totals
// leave the kernel; no PIDs, names or arguments are recorded.
//
// The few kernel fields read are declared below with CO-RE relocations, so the object loads on
// any kernel with BTF without a generated vmlinux.h.

#include <stdbool.h>
#include <linux/types.h>
#include <linux/bpf.h>
#include <bpf/bpf_helpers.h>
#include <bpf/bpf_core_read.h>
#include <bpf/bpf_tracing.h>

struct kernfs_node {
	__u64 id;
} __attribute__((preserve_access_index));

struct cgroup {
	struct kernfs_node *kn;
} __attribute__((preserve_access_index));

struct css_set {
	struct cgroup *dfl_cgrp;
} __attribute__((preserve_access_index));

struct task_struct {
	int pid;
	struct css_set *cgroups;
} __attribute__((preserve_access_index));

// Bounded: least recently updated cgroups (usually removed ones) are evicted first.
struct {
	__uint(type, BPF_MAP_TYPE_LRU_HASH);
	__uint(max_entries, 16384);
	__type(key, __u64);
	__type(value, __u64);
} wakeups SEC(".maps");

SEC("tp_btf/sched_switch")
int BPF_PROG(count_idle_exits, bool preempt, struct task_struct *prev, struct task_struct *next)
{
	// Every CPU's idle task has PID 0.
	if (BPF_CORE_READ(prev, pid) != 0 || BPF_CORE_READ(next, pid) == 0)
		return 0;
	__u64 cgroup = BPF_CORE_READ(next, cgroups, dfl_cgrp, kn, id);
	__u64 *count = bpf_map_lookup_elem(&wakeups, &cgroup);
	if (count) {
		__sync_fetch_and_add(count, 1);
	} else {
		__u64 one = 1;
		// Another CPU may insert the same key first; losing one count is fine.
		bpf_map_update_elem(&wakeups, &cgroup, &one, BPF_NOEXIST);
	}
	return 0;
}

char LICENSE[] SEC("license") = "GPL";
