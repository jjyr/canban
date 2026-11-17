// Copyright 2026 JJy <jjyruby@gmail.com>. SPDX-License-Identifier: Apache-2.0
#include <os/signpost.h>
#include <pthread.h>
#include <stdint.h>
static os_log_t log_handle;
static pthread_once_t once = PTHREAD_ONCE_INIT;
static void initialize(void) {
    log_handle = os_log_create("com.canban.profiler", OS_LOG_CATEGORY_POINTS_OF_INTEREST);
}
uint64_t canban_span_begin(const char *name) {
    pthread_once(&once, initialize);
    os_signpost_id_t id = os_signpost_id_generate(log_handle);
    os_signpost_interval_begin(log_handle, id, "Canban", "%{public}s", name);
    return id;
}
void canban_span_end(uint64_t id, const char *name, uint64_t items, uint64_t bytes) {
    os_signpost_interval_end(log_handle, id, "Canban", "%{public}s items=%llu bytes=%llu",
        name, (unsigned long long)items, (unsigned long long)bytes);
}
