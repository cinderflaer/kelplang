; ModuleID = 'kelp'
source_filename = "kelp"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%kelp_gc_type = type { i32, i32, i32, i32, i32, ptr, i32, i32, ptr }
%kelp_Bytes = type { ptr, i64, i64 }
%kelp_String = type { ptr, i64, i64 }
%kelp_View = type { ptr, i64 }
%tag_Item = type { %kelp_String, i32 }
%kelp_List__Item = type { ptr, i64, i64 }
%kelp_List__String = type { ptr, i64, i64 }
%kelp_Box__i32 = type { ptr }
%kelp_Map__String__Item = type { ptr, i64, i64, i64, ptr, i64, i64 }
%kelp_Map__String__Item_entry = type { %kelp_String, %tag_Item, i8 }
%kelp_Closure__i32__i32 = type { ptr, ptr, ptr }
%kelp_Env____funlit1 = type { %kelp_Box__i32 }

@kelp_argc = external global i32
@kelp_argv = external global ptr
@kelp_caps_containers_env = external global ptr
@kelp_caps_containers_frames = external global ptr
@kelp_caps_containers_fs = external global ptr
@kelp_caps_containers_irq = external global ptr
@kelp_caps_containers_net = external global ptr
@kelp_caps_containers_process = external global ptr
@kelp_caps_containers_rand = external global ptr
@kelp_caps_containers_time = external global ptr
@kelp_caps_containers_ui = external global ptr
@panic.msg = private unnamed_addr constant [31 x i8] c"Bytes.get: index out of bounds\00", align 1
@panic.msg.1 = private unnamed_addr constant [31 x i8] c"Bytes.set: index out of bounds\00", align 1
@panic.msg.2 = private unnamed_addr constant [30 x i8] c"View.get: index out of bounds\00", align 1
@kelp_gc_ptrs_Item = internal constant [1 x i32] zeroinitializer
@kelp_gc_type_Item = internal constant %kelp_gc_type { i32 1, i32 32, i32 8, i32 0, i32 1, ptr @kelp_gc_ptrs_Item, i32 0, i32 0, ptr null }
@panic.msg.3 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.4 = private unnamed_addr constant [30 x i8] c"List.get: index out of bounds\00", align 1
@panic.msg.5 = private unnamed_addr constant [28 x i8] c"List.pop: the list is empty\00", align 1
@panic.msg.6 = private unnamed_addr constant [33 x i8] c"List.insert: index out of bounds\00", align 1
@panic.msg.7 = private unnamed_addr constant [33 x i8] c"List.remove: index out of bounds\00", align 1
@panic.msg.8 = private unnamed_addr constant [32 x i8] c"List.slice: range out of bounds\00", align 1
@kelp_gc_ptrs_String = internal constant [1 x i32] zeroinitializer
@kelp_gc_type_String = internal constant %kelp_gc_type { i32 2, i32 24, i32 8, i32 0, i32 1, ptr @kelp_gc_ptrs_String, i32 0, i32 0, ptr null }
@panic.msg.9 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.10 = private unnamed_addr constant [30 x i8] c"List.get: index out of bounds\00", align 1
@panic.msg.11 = private unnamed_addr constant [28 x i8] c"List.pop: the list is empty\00", align 1
@panic.msg.12 = private unnamed_addr constant [33 x i8] c"List.insert: index out of bounds\00", align 1
@panic.msg.13 = private unnamed_addr constant [33 x i8] c"List.remove: index out of bounds\00", align 1
@panic.msg.14 = private unnamed_addr constant [32 x i8] c"List.slice: range out of bounds\00", align 1
@kelp_gc_type_i32 = internal constant %kelp_gc_type { i32 3, i32 4, i32 4, i32 1, i32 0, ptr null, i32 0, i32 0, ptr null }
@panic.msg.15 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.16 = private unnamed_addr constant [26 x i8] c"Box.get: the box is empty\00", align 1
@panic.msg.17 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.18 = private unnamed_addr constant [27 x i8] c"Box.take: the box is empty\00", align 1
@kelp_gc_ptrs_map_entry.kelp_Map__String__Item = internal constant [2 x i32] [i32 0, i32 24]
@kelp_gc_type_map_entry.kelp_Map__String__Item = internal constant %kelp_gc_type { i32 4, i32 64, i32 8, i32 0, i32 2, ptr @kelp_gc_ptrs_map_entry.kelp_Map__String__Item, i32 0, i32 0, ptr null }
@panic.msg.19 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@kelp_gc_type_u32 = internal constant %kelp_gc_type { i32 5, i32 4, i32 4, i32 1, i32 0, ptr null, i32 0, i32 0, ptr null }
@panic.msg.20 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.21 = private unnamed_addr constant [23 x i8] c"Map.get: key not found\00", align 1
@panic.msg.22 = private unnamed_addr constant [26 x i8] c"Map.remove: key not found\00", align 1
@panic.msg.23 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@str = private unnamed_addr constant [6 x i8] c"alpha\00", align 1
@str.24 = private unnamed_addr constant [5 x i8] c"%zu\0A\00", align 1
@str.25 = private unnamed_addr constant [2 x i8] c"a\00", align 1
@str.26 = private unnamed_addr constant [6 x i8] c"alpha\00", align 1
@"kelp_gc_ptrs_env.#funlit1" = internal constant [1 x i32] zeroinitializer
@"kelp_gc_type_env.#funlit1" = internal constant %kelp_gc_type { i32 6, i32 8, i32 8, i32 0, i32 1, ptr @"kelp_gc_ptrs_env.#funlit1", i32 0, i32 0, ptr null }
@panic.msg.27 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@str.28 = private unnamed_addr constant [4 x i8] c"%d\0A\00", align 1

declare i32 @printf(ptr, ...)

declare void @kelp_panic(ptr)

declare void @kelp_assert(i1 zeroext)

declare ptr @kelp_thread_new(i64, i64)

declare void @kelp_thread_start(ptr, ptr, ptr, ptr, ptr)

declare i32 @kelp_thread_wait(ptr)

declare ptr @kelp_thread_arg(ptr)

declare ptr @kelp_thread_result(ptr)

declare ptr @kelp_thread_message(ptr)

declare void @kelp_thread_dispose(ptr)

declare void @kelp_thread_drop(ptr)

declare ptr @kelp_mutex_new(ptr, i64, ptr)

declare void @kelp_mutex_run(ptr, ptr, ptr, ptr, ptr)

declare void @kelp_mutex_into(ptr, ptr)

declare void @kelp_mutex_drop(ptr)

declare ptr @kelp_mutex_share(ptr)

declare void @kelp_mutex_swap(ptr, ptr, ptr)

declare ptr @kelp_condvar_share(ptr)

declare ptr @kelp_rwlock_new(ptr, i64, ptr)

declare void @kelp_rwlock_read(ptr, ptr, ptr, ptr, ptr)

declare void @kelp_rwlock_write(ptr, ptr, ptr, ptr, ptr)

declare i32 @kelp_rwlock_try_read(ptr, ptr, ptr, ptr, ptr)

declare i32 @kelp_rwlock_try_write(ptr, ptr, ptr, ptr, ptr)

declare void @kelp_rwlock_into(ptr, ptr)

declare void @kelp_rwlock_drop(ptr)

declare ptr @kelp_condvar_new()

declare void @kelp_condvar_notify_one(ptr)

declare void @kelp_condvar_notify_all(ptr)

declare void @kelp_condvar_drop(ptr)

declare void @kelp_mutex_wait_while(ptr, ptr, ptr, ptr, ptr, i32, ptr, ptr, ptr, ptr)

declare i32 @kelp_mutex_wait_while_timeout(ptr, ptr, ptr, ptr, ptr, i32, i64, ptr, ptr, ptr, ptr)

declare void @kelp_Bytes__empty(ptr)

declare void @kelp_Bytes__from_str(ptr, ptr)

declare void @kelp_Bytes__clone(ptr, ptr)

define internal i64 @kelp_Bytes__len(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  ret i64 %len1
}

define internal zeroext i1 @kelp_Bytes__is_empty(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  ret i1 %empty
}

define internal zeroext i8 @kelp_Bytes__get(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %panic, label %in.bounds

panic:                                            ; preds = %entry
  call void @kelp_panic(ptr @panic.msg)
  unreachable

in.bounds:                                        ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 0
  %data2 = load ptr, ptr %data, align 8
  %at = getelementptr i8, ptr %data2, i64 %1
  %byte = load i8, ptr %at, align 1
  ret i8 %byte
}

define internal void @kelp_Bytes__set(ptr %0, i64 %1, i8 zeroext %2) {
entry:
  %len = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %panic, label %in.bounds

panic:                                            ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.1)
  unreachable

in.bounds:                                        ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 0
  %data2 = load ptr, ptr %data, align 8
  %at = getelementptr i8, ptr %data2, i64 %1
  store i8 %2, ptr %at, align 1
  ret void
}

define internal void @kelp_Bytes__push(ptr %0, i8 zeroext %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %len2 = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  %len3 = load i64, ptr %len2, align 8
  %cap = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 2
  %cap4 = load i64, ptr %cap, align 8
  %need = add i64 %len3, 1
  %fits = icmp ule i64 %need, %cap4
  br i1 %fits, label %reserved, label %grow

grow:                                             ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 0
  %cap5 = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 2
  call void @kelp_buf_reserve(ptr %data, i64 %len3, ptr %cap5, i64 1)
  br label %reserved

reserved:                                         ; preds = %grow, %entry
  %data6 = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 0
  %data7 = load ptr, ptr %data6, align 8
  %at = getelementptr i8, ptr %data7, i64 %len1
  store i8 %1, ptr %at, align 1
  %len18 = add i64 %len1, 1
  %len9 = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 1
  store i64 %len18, ptr %len9, align 8
  %data10 = getelementptr inbounds nuw %kelp_Bytes, ptr %0, i32 0, i32 0
  %data11 = load ptr, ptr %data10, align 8
  %term = getelementptr i8, ptr %data11, i64 %len18
  store i8 0, ptr %term, align 1
  ret void
}

declare void @kelp_Bytes__append(ptr, ptr)

declare void @kelp_Bytes__reserve(ptr, i64)

declare void @kelp_Bytes__clear(ptr)

declare ptr @kelp_Bytes__as_str(ptr)

declare void @kelp_bytes_drop(ptr)

declare void @kelp_String__empty(ptr)

declare void @kelp_String__from_str(ptr, ptr)

declare void @kelp_String__clone(ptr, ptr)

declare void @kelp_String__with_capacity(ptr, i64)

define internal i64 @kelp_String__len(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  ret i64 %len1
}

declare i64 @kelp_String__char_count(ptr)

define internal zeroext i1 @kelp_String__is_empty(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  ret i1 %empty
}

declare ptr @kelp_String__as_str(ptr)

declare %kelp_View @kelp_String__view(ptr)

define internal void @kelp_String__push(ptr %0, i8 zeroext %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %len2 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len3 = load i64, ptr %len2, align 8
  %cap = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  %cap4 = load i64, ptr %cap, align 8
  %need = add i64 %len3, 1
  %fits = icmp ule i64 %need, %cap4
  br i1 %fits, label %reserved, label %grow

grow:                                             ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %cap5 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  call void @kelp_buf_reserve(ptr %data, i64 %len3, ptr %cap5, i64 1)
  br label %reserved

reserved:                                         ; preds = %grow, %entry
  %data6 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %data7 = load ptr, ptr %data6, align 8
  %at = getelementptr i8, ptr %data7, i64 %len1
  store i8 %1, ptr %at, align 1
  %len18 = add i64 %len1, 1
  %len9 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  store i64 %len18, ptr %len9, align 8
  %data10 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %data11 = load ptr, ptr %data10, align 8
  %term = getelementptr i8, ptr %data11, i64 %len18
  store i8 0, ptr %term, align 1
  ret void
}

define internal void @kelp_String__push_str(ptr %0, ptr %1) {
entry:
  %n = call i64 @strlen(ptr %1)
  %len0 = icmp eq i64 %n, 0
  br i1 %len0, label %ret, label %copy

ret:                                              ; preds = %reserved, %entry
  ret void

copy:                                             ; preds = %entry
  %len = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %len2 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len3 = load i64, ptr %len2, align 8
  %cap = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  %cap4 = load i64, ptr %cap, align 8
  %need = add i64 %len3, %n
  %fits = icmp ule i64 %need, %cap4
  br i1 %fits, label %reserved, label %grow

grow:                                             ; preds = %copy
  %data = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %cap5 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  call void @kelp_buf_reserve(ptr %data, i64 %len3, ptr %cap5, i64 %n)
  br label %reserved

reserved:                                         ; preds = %grow, %copy
  %data6 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %data7 = load ptr, ptr %data6, align 8
  %dst = getelementptr i8, ptr %data7, i64 %len1
  %2 = call ptr @memcpy(ptr %dst, ptr %1, i64 %n)
  %len18 = add i64 %len1, %n
  %len9 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  store i64 %len18, ptr %len9, align 8
  %data10 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %data11 = load ptr, ptr %data10, align 8
  %term = getelementptr i8, ptr %data11, i64 %len18
  store i8 0, ptr %term, align 1
  br label %ret
}

define internal void @kelp_String__reserve(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %cap = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  %cap2 = load i64, ptr %cap, align 8
  %need = add i64 %len1, %1
  %fits = icmp ule i64 %need, %cap2
  br i1 %fits, label %reserved, label %grow

grow:                                             ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %cap3 = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 2
  call void @kelp_buf_reserve(ptr %data, i64 %len1, ptr %cap3, i64 %1)
  br label %reserved

reserved:                                         ; preds = %grow, %entry
  ret void
}

declare void @kelp_String__clear(ptr)

declare void @kelp_string_drop(ptr)

declare void @kelp_buf_reserve(ptr, i64, ptr, i64)

declare %kelp_View @kelp_str__view(ptr)

define internal i64 @kelp_View__len(%kelp_View %0) {
entry:
  %len = extractvalue %kelp_View %0, 1
  ret i64 %len
}

define internal zeroext i1 @kelp_View__is_empty(%kelp_View %0) {
entry:
  %len = extractvalue %kelp_View %0, 1
  %empty = icmp eq i64 %len, 0
  ret i1 %empty
}

define internal zeroext i8 @kelp_View__get(%kelp_View %0, i64 %1) {
entry:
  %len = extractvalue %kelp_View %0, 1
  %oob = icmp uge i64 %1, %len
  br i1 %oob, label %panic, label %in.bounds

panic:                                            ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.2)
  unreachable

in.bounds:                                        ; preds = %entry
  %ptr = extractvalue %kelp_View %0, 0
  %at = getelementptr i8, ptr %ptr, i64 %1
  %byte = load i8, ptr %at, align 1
  ret i8 %byte
}

declare %kelp_View @kelp_View__slice(%kelp_View, i64, i64)

declare zeroext i1 @kelp_View__eq(%kelp_View, %kelp_View)

declare void @kelp_View__to_string(ptr, %kelp_View)

declare i64 @kelp_map_hash(ptr, i64)

declare i64 @kelp_map_mix(i64)

declare zeroext i1 @kelp_map_key_eq(ptr, ptr, i64)

declare i64 @kelp_map_combine(i64, i64)

declare void @kelp_String__from_bytes_ptr(ptr, ptr)

declare void @kelp_String__into_bytes_ptr(ptr, ptr)

declare ptr @malloc(i64)

declare ptr @realloc(ptr, i64)

declare void @free(ptr)

declare ptr @memcpy(ptr, ptr, i64)

declare i32 @memcmp(ptr, ptr, i64)

declare ptr @memset(ptr, i32, i64)

declare i64 @strlen(ptr)

declare i32 @atexit(ptr)

declare void @kelp_cap_check_path(ptr, ptr, ptr, i64, ptr, ptr)

declare void @kelp_cap_check_name(ptr, ptr, ptr, i64, ptr, ptr)

declare void @kelp_cap_check_host(ptr, ptr, ptr, i64, ptr, ptr)

declare void @kelp_cap_check_hostport(ptr, ptr, i16 zeroext, ptr, i64, ptr, ptr)

declare void @kelp_cap_check_port(ptr, i16 zeroext, ptr, i64, ptr, ptr)

declare void @kelp_cap_deny(ptr, ptr, ptr, i64, ptr, ptr)

declare ptr @kelp_gc_alloc(ptr)

declare ptr @kelp_gc_alloc_array(ptr, i64)

declare void @kelp_gc_init(i64, i64)

declare void @kelp_gc_safepoint()

declare void @kelp_gc_thread_attach()

declare void @kelp_gc_thread_detach()

define void @kelp_clone_Item(ptr %0, ptr %1) {
entry:
  %src = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 0
  %dst = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 0
  call void @kelp_String__clone(ptr %dst, ptr %src)
  %src1 = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 1
  %dst2 = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 1
  %copy = load i32, ptr %src1, align 4
  store i32 %copy, ptr %dst2, align 4
  ret void
}

define i32 @kelp_cmp_Item(ptr %0, ptr %1) {
entry:
  %a = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 0
  %b = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 0
  %alen = getelementptr inbounds nuw %kelp_String, ptr %a, i32 0, i32 1
  %blen = getelementptr inbounds nuw %kelp_String, ptr %b, i32 0, i32 1
  %alen1 = load i64, ptr %alen, align 8
  %blen2 = load i64, ptr %blen, align 8
  %lt = icmp ult i64 %alen1, %blen2
  %n = select i1 %lt, i64 %alen1, i64 %blen2
  %nz = icmp ugt i64 %n, 0
  br i1 %nz, label %cmp.bytes, label %cmp.after

cmp.bytes:                                        ; preds = %entry
  %ad = getelementptr inbounds nuw %kelp_String, ptr %a, i32 0, i32 0
  %ad3 = load ptr, ptr %ad, align 8
  %bd = getelementptr inbounds nuw %kelp_String, ptr %b, i32 0, i32 0
  %bd4 = load ptr, ptr %bd, align 8
  %memcmp = call i32 @memcmp(ptr %ad3, ptr %bd4, i64 %n)
  %ne = icmp ne i32 %memcmp, 0
  br i1 %ne, label %cmp.diff, label %cmp.keep

cmp.after:                                        ; preds = %cmp.keep, %entry
  %lendiff = icmp ne i64 %alen1, %blen2
  br i1 %lendiff, label %cmp.len, label %cmp.cont

cmp.diff:                                         ; preds = %cmp.bytes
  %neg = icmp slt i32 %memcmp, 0
  %sign = select i1 %neg, i32 -1, i32 1
  ret i32 %sign

cmp.keep:                                         ; preds = %cmp.bytes
  br label %cmp.after

cmp.len:                                          ; preds = %cmp.after
  %alenlt = icmp ult i64 %alen1, %blen2
  %lsign = select i1 %alenlt, i32 -1, i32 1
  ret i32 %lsign

cmp.cont:                                         ; preds = %cmp.after
  %a5 = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 1
  %b6 = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 1
  %av = load i32, ptr %a5, align 4
  %bv = load i32, ptr %b6, align 4
  %ne7 = icmp ne i32 %av, %bv
  %lt8 = icmp slt i32 %av, %bv
  br i1 %ne7, label %cmp.diff9, label %cmp.cont10

cmp.diff9:                                        ; preds = %cmp.cont
  %sign11 = select i1 %lt8, i32 -1, i32 1
  ret i32 %sign11

cmp.cont10:                                       ; preds = %cmp.cont
  ret i32 0
}

define i64 @kelp_hash_Item(ptr %0) {
entry:
  %h = alloca i64, align 8
  store i64 0, ptr %h, align 8
  %f = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 0
  %d = getelementptr inbounds nuw %kelp_String, ptr %f, i32 0, i32 0
  %d1 = load ptr, ptr %d, align 8
  %n = getelementptr inbounds nuw %kelp_String, ptr %f, i32 0, i32 1
  %n2 = load i64, ptr %n, align 8
  %maphash.h = alloca i64, align 8
  store i64 1469598103934665603, ptr %maphash.h, align 8
  %maphash.idx = alloca i64, align 8
  store i64 0, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.cond:                                     ; preds = %maphash.body, %entry
  %maphash.i = load i64, ptr %maphash.idx, align 8
  %maphash.more = icmp ult i64 %maphash.i, %n2
  br i1 %maphash.more, label %maphash.body, label %maphash.after

maphash.body:                                     ; preds = %maphash.cond
  %maphash.cptr = getelementptr i8, ptr %d1, i64 %maphash.i
  %maphash.c = load i8, ptr %maphash.cptr, align 1
  %maphash.cw = zext i8 %maphash.c to i64
  %maphash.h1 = load i64, ptr %maphash.h, align 8
  %maphash.hx = xor i64 %maphash.h1, %maphash.cw
  %maphash.hm = mul i64 %maphash.hx, 1099511628211
  store i64 %maphash.hm, ptr %maphash.h, align 8
  %maphash.next = add i64 %maphash.i, 1
  store i64 %maphash.next, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.after:                                    ; preds = %maphash.cond
  %maphash.result = load i64, ptr %maphash.h, align 8
  %h3 = load i64, ptr %h, align 8
  %mix.s1 = lshr i64 %maphash.result, 33
  %mix.x1 = xor i64 %maphash.result, %mix.s1
  %mix.x2 = mul i64 %mix.x1, -49064778989728563
  %mix.s2 = lshr i64 %mix.x2, 33
  %mix.x3 = xor i64 %mix.x2, %mix.s2
  %combine.h1 = xor i64 %h3, %mix.x3
  %combine.h2 = mul i64 %combine.h1, 1099511628211
  store i64 %combine.h2, ptr %h, align 8
  %f4 = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 1
  %v = load i32, ptr %f4, align 4
  %kw = sext i32 %v to i64
  %mix.s15 = lshr i64 %kw, 33
  %mix.x16 = xor i64 %kw, %mix.s15
  %mix.x27 = mul i64 %mix.x16, -49064778989728563
  %mix.s28 = lshr i64 %mix.x27, 33
  %mix.x39 = xor i64 %mix.x27, %mix.s28
  %h10 = load i64, ptr %h, align 8
  %mix.s111 = lshr i64 %mix.x39, 33
  %mix.x112 = xor i64 %mix.x39, %mix.s111
  %mix.x213 = mul i64 %mix.x112, -49064778989728563
  %mix.s214 = lshr i64 %mix.x213, 33
  %mix.x315 = xor i64 %mix.x213, %mix.s214
  %combine.h116 = xor i64 %h10, %mix.x315
  %combine.h217 = mul i64 %combine.h116, 1099511628211
  store i64 %combine.h217, ptr %h, align 8
  %h18 = load i64, ptr %h, align 8
  ret i64 %h18
}

define i1 @kelp_eq_Item(ptr %0, ptr %1) {
entry:
  %a = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 0
  %b = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 0
  %d = getelementptr inbounds nuw %kelp_String, ptr %b, i32 0, i32 0
  %d1 = load ptr, ptr %d, align 8
  %n = getelementptr inbounds nuw %kelp_String, ptr %b, i32 0, i32 1
  %n2 = load i64, ptr %n, align 8
  %eq = call i1 @kelp_map_key_eq(ptr %a, ptr %d1, i64 %n2)
  %acc = and i1 true, %eq
  %a3 = getelementptr inbounds nuw %tag_Item, ptr %0, i32 0, i32 1
  %b4 = getelementptr inbounds nuw %tag_Item, ptr %1, i32 0, i32 1
  %av = load i32, ptr %a3, align 4
  %bv = load i32, ptr %b4, align 4
  %eq5 = icmp eq i32 %av, %bv
  %acc6 = and i1 %acc, %eq5
  ret i1 %acc6
}

define internal %kelp_List__Item @kelp_List__Item__empty() {
entry:
  %l = alloca %kelp_List__Item, align 8
  store %kelp_List__Item zeroinitializer, ptr %l, align 8
  %ret = load %kelp_List__Item, ptr %l, align 8
  ret %kelp_List__Item %ret
}

define internal void @kelp_List__Item__reserve(ptr %0, i64 %1) {
entry:
  %cap = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 2
  %cap1 = load i64, ptr %cap, align 8
  %enough = icmp ule i64 %1, %cap1
  br i1 %enough, label %ret, label %grow

ret:                                              ; preds = %alloc.ok, %entry
  ret void

grow:                                             ; preds = %entry
  %cap2 = alloca i64, align 8
  %zero = icmp eq i64 %cap1, 0
  %start = select i1 %zero, i64 4, i64 %cap1
  store i64 %start, ptr %cap2, align 8
  br label %grow.cond

grow.cond:                                        ; preds = %grow.body, %grow
  %cap3 = load i64, ptr %cap2, align 8
  %need = icmp ult i64 %cap3, %1
  br i1 %need, label %grow.body, label %grow.end

grow.body:                                        ; preds = %grow.cond
  %cap4 = load i64, ptr %cap2, align 8
  %cap25 = mul i64 %cap4, 2
  store i64 %cap25, ptr %cap2, align 8
  br label %grow.cond

grow.end:                                         ; preds = %grow.cond
  %cap6 = load i64, ptr %cap2, align 8
  %bytes = mul i64 %cap6, 32
  %old.bytes = mul i64 %cap1, 32
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data7 = load ptr, ptr %data, align 8
  %gc.arr = call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_Item, i64 %cap6)
  %oom = icmp eq ptr %gc.arr, null
  br i1 %oom, label %oom8, label %alloc.ok

oom8:                                             ; preds = %grow.end
  call void @kelp_panic(ptr @panic.msg.3)
  unreachable

alloc.ok:                                         ; preds = %grow.end
  %2 = call ptr @memcpy(ptr %gc.arr, ptr %data7, i64 %old.bytes)
  %data9 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  store ptr %gc.arr, ptr %data9, align 8
  %cap10 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 2
  store i64 %cap6, ptr %cap10, align 8
  br label %ret
}

define internal %kelp_List__Item @kelp_List__Item__with_capacity(i64 %0) {
entry:
  %l = alloca %kelp_List__Item, align 8
  store %kelp_List__Item zeroinitializer, ptr %l, align 8
  call void @kelp_List__Item__reserve(ptr %l, i64 %0)
  %wc = load %kelp_List__Item, ptr %l, align 8
  ret %kelp_List__Item %wc
}

define internal %kelp_List__Item @kelp_List__Item__of(%tag_Item %0) {
entry:
  %l = alloca %kelp_List__Item, align 8
  store %kelp_List__Item zeroinitializer, ptr %l, align 8
  call void @kelp_List__Item__reserve(ptr %l, i64 1)
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %l, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data1, i64 0
  store %tag_Item %0, ptr %at, align 8
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %l, i32 0, i32 1
  store i64 1, ptr %len, align 8
  %of = load %kelp_List__Item, ptr %l, align 8
  ret %kelp_List__Item %of
}

define internal i64 @kelp_List__Item__len(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  ret i64 %len1
}

define internal i1 @kelp_List__Item__is_empty(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  ret i1 %empty
}

define internal void @kelp_List__Item__push(ptr %0, %tag_Item %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %want = add i64 %len1, 1
  call void @kelp_List__Item__reserve(ptr %0, i64 %want)
  %len2 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len3 = load i64, ptr %len2, align 8
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data4, i64 %len3
  store %tag_Item %1, ptr %at, align 8
  %len15 = add i64 %len3, 1
  %len6 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  ret void
}

define internal void @kelp_List__Item__bounds(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %oob2, label %inbounds

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.4)
  unreachable

inbounds:                                         ; preds = %entry
  ret void
}

define internal ptr @kelp_List__Item__get_const(ptr %0, i64 %1) {
entry:
  call void @kelp_List__Item__bounds(ptr %0, i64 %1)
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data1, i64 %1
  ret ptr %at
}

define internal void @kelp_List__Item__set(ptr %0, i64 %1, %tag_Item %2) {
entry:
  call void @kelp_List__Item__bounds(ptr %0, i64 %1)
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data1, i64 %1
  store %tag_Item %2, ptr %at, align 8
  ret void
}

define internal %tag_Item @kelp_List__Item__pop(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  br i1 %empty, label %empty2, label %pop

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.5)
  unreachable

pop:                                              ; preds = %entry
  %last = sub i64 %len1, 1
  %len3 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  store i64 %last, ptr %len3, align 8
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data4, i64 %last
  %pop5 = load %tag_Item, ptr %at, align 8
  ret %tag_Item %pop5
}

define internal void @kelp_List__Item__clear(ptr %0) {
entry:
  %i = alloca i64, align 8
  store i64 0, ptr %i, align 8
  br label %clear.cond

clear.cond:                                       ; preds = %clear.body, %entry
  %i1 = load i64, ptr %i, align 8
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len2 = load i64, ptr %len, align 8
  %more = icmp ult i64 %i1, %len2
  br i1 %more, label %clear.body, label %clear.end

clear.body:                                       ; preds = %clear.cond
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %at = getelementptr %tag_Item, ptr %data3, i64 %i1
  %i14 = add i64 %i1, 1
  store i64 %i14, ptr %i, align 8
  br label %clear.cond

clear.end:                                        ; preds = %clear.cond
  %len5 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  store i64 0, ptr %len5, align 8
  ret void
}

define internal void @kelp_List__Item__reverse(ptr %0) {
entry:
  %i = alloca i64, align 8
  %j = alloca i64, align 8
  store i64 0, ptr %i, align 8
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  store i64 %len1, ptr %j, align 8
  br label %rev.cond

rev.cond:                                         ; preds = %rev.body, %entry
  %i2 = load i64, ptr %i, align 8
  %j3 = load i64, ptr %j, align 8
  %i1 = add i64 %i2, 1
  %more = icmp ult i64 %i1, %j3
  br i1 %more, label %rev.body, label %rev.end

rev.body:                                         ; preds = %rev.cond
  %j1 = sub i64 %j3, 1
  store i64 %j1, ptr %j, align 8
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %ai = getelementptr %tag_Item, ptr %data4, i64 %i2
  %aj = getelementptr %tag_Item, ptr %data4, i64 %j1
  %t = load %tag_Item, ptr %ai, align 8
  %u = load %tag_Item, ptr %aj, align 8
  store %tag_Item %u, ptr %ai, align 8
  store %tag_Item %t, ptr %aj, align 8
  %i15 = add i64 %i2, 1
  store i64 %i15, ptr %i, align 8
  br label %rev.cond

rev.end:                                          ; preds = %rev.cond
  ret void
}

define internal i32 @kelp_List__Item__cmp(ptr %0, ptr %1) {
entry:
  %a = load %tag_Item, ptr %0, align 8
  %b = load %tag_Item, ptr %1, align 8
  %cmp = call i32 @kelp_cmp_Item(ptr %0, ptr %1)
  ret i32 %cmp
}

define internal void @kelp_List__Item__sift_down(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %root = alloca i64, align 8
  store i64 %2, ptr %root, align 8
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  br label %sift.cond

sift.cond:                                        ; preds = %sift.swap, %entry
  %root2 = load i64, ptr %root, align 8
  %twice = mul i64 %root2, 2
  %child = add i64 %twice, 1
  %in_heap = icmp ult i64 %child, %3
  br i1 %in_heap, label %sift.pick, label %sift.end

sift.pick:                                        ; preds = %sift.cond
  %child1 = add i64 %child, 1
  %has_right = icmp ult i64 %child1, %3
  br i1 %has_right, label %sift.cmpchild, label %sift.afterpick

sift.compare:                                     ; preds = %sift.afterpick
  %root_abs = add i64 %1, %root2
  %chosen_abs = add i64 %1, %chosen
  %rootp = getelementptr %tag_Item, ptr %data1, i64 %root_abs
  %childp = getelementptr %tag_Item, ptr %data1, i64 %chosen_abs
  %cmp3 = call i32 @kelp_List__Item__cmp(ptr %childp, ptr %rootp)
  %gt = icmp sgt i32 %cmp3, 0
  br i1 %gt, label %sift.swap, label %sift.end

sift.end:                                         ; preds = %sift.compare, %sift.cond
  ret void

sift.cmpchild:                                    ; preds = %sift.pick
  %child_abs = add i64 %1, %child
  %child1_abs = add i64 %1, %child1
  %l = getelementptr %tag_Item, ptr %data1, i64 %child_abs
  %r = getelementptr %tag_Item, ptr %data1, i64 %child1_abs
  %cmp = call i32 @kelp_List__Item__cmp(ptr %l, ptr %r)
  %rb = icmp slt i32 %cmp, 0
  %bigger = select i1 %rb, i64 %child1, i64 %child
  br label %sift.afterpick

sift.afterpick:                                   ; preds = %sift.cmpchild, %sift.pick
  %chosen = phi i64 [ %child, %sift.pick ], [ %bigger, %sift.cmpchild ]
  br label %sift.compare

sift.swap:                                        ; preds = %sift.compare
  %pv = load %tag_Item, ptr %rootp, align 8
  %cv = load %tag_Item, ptr %childp, align 8
  store %tag_Item %cv, ptr %rootp, align 8
  store %tag_Item %pv, ptr %childp, align 8
  store i64 %chosen, ptr %root, align 8
  br label %sift.cond
}

define internal void @kelp_List__Item__heapsort_range(ptr %0, i64 %1, i64 %2) {
entry:
  %n = sub i64 %2, %1
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %start = alloca i64, align 8
  %half = udiv i64 %n, 2
  store i64 %half, ptr %start, align 8
  br label %hr.heap.cond

hr.heap.cond:                                     ; preds = %hr.heap.body, %entry
  %s = load i64, ptr %start, align 8
  %spos = icmp ugt i64 %s, 0
  br i1 %spos, label %hr.heap.body, label %hr.heap.end

hr.heap.body:                                     ; preds = %hr.heap.cond
  %s1 = sub i64 %s, 1
  store i64 %s1, ptr %start, align 8
  call void @kelp_List__Item__sift_down(ptr %0, i64 %1, i64 %s1, i64 %n)
  br label %hr.heap.cond

hr.heap.end:                                      ; preds = %hr.heap.cond
  %end = alloca i64, align 8
  store i64 %n, ptr %end, align 8
  br label %hr.sort.cond

hr.sort.cond:                                     ; preds = %hr.sort.body, %hr.heap.end
  %e = load i64, ptr %end, align 8
  %epos = icmp ugt i64 %e, 1
  br i1 %epos, label %hr.sort.body, label %hr.sort.end

hr.sort.body:                                     ; preds = %hr.sort.cond
  %e1 = sub i64 %e, 1
  store i64 %e1, ptr %end, align 8
  %lo_e1 = add i64 %1, %e1
  %d0 = getelementptr %tag_Item, ptr %data1, i64 %1
  %de = getelementptr %tag_Item, ptr %data1, i64 %lo_e1
  %tv = load %tag_Item, ptr %d0, align 8
  %ev = load %tag_Item, ptr %de, align 8
  store %tag_Item %ev, ptr %d0, align 8
  store %tag_Item %tv, ptr %de, align 8
  call void @kelp_List__Item__sift_down(ptr %0, i64 %1, i64 0, i64 %e1)
  br label %hr.sort.cond

hr.sort.end:                                      ; preds = %hr.sort.cond
  ret void
}

define internal void @kelp_List__Item__insertion_sort(ptr %0, i64 %1, i64 %2) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %i = alloca i64, align 8
  %lo1 = add i64 %1, 1
  store i64 %lo1, ptr %i, align 8
  br label %ins.cond

ins.cond:                                         ; preds = %ins.shift.end, %entry
  %i2 = load i64, ptr %i, align 8
  %more = icmp ult i64 %i2, %2
  br i1 %more, label %ins.body, label %ins.end

ins.body:                                         ; preds = %ins.cond
  %at_i = getelementptr %tag_Item, ptr %data1, i64 %i2
  %key = alloca %tag_Item, align 8
  %keyv = load %tag_Item, ptr %at_i, align 8
  store %tag_Item %keyv, ptr %key, align 8
  %j = alloca i64, align 8
  store i64 %i2, ptr %j, align 8
  br label %ins.shift.cond

ins.end:                                          ; preds = %ins.cond
  ret void

ins.shift.cond:                                   ; preds = %ins.shift.body, %ins.body
  %j3 = load i64, ptr %j, align 8
  %jgt = icmp ugt i64 %j3, %1
  br i1 %jgt, label %ins.shift.check, label %ins.shift.end

ins.shift.check:                                  ; preds = %ins.shift.cond
  %j1 = sub i64 %j3, 1
  %prevp = getelementptr %tag_Item, ptr %data1, i64 %j1
  %cmp = call i32 @kelp_List__Item__cmp(ptr %prevp, ptr %key)
  %gtc = icmp sgt i32 %cmp, 0
  br i1 %gtc, label %ins.shift.body, label %ins.shift.end

ins.shift.end:                                    ; preds = %ins.shift.check, %ins.shift.cond
  %atj_final = getelementptr %tag_Item, ptr %data1, i64 %j3
  %keyv_final = load %tag_Item, ptr %key, align 8
  store %tag_Item %keyv_final, ptr %atj_final, align 8
  %i1 = add i64 %i2, 1
  store i64 %i1, ptr %i, align 8
  br label %ins.cond

ins.shift.body:                                   ; preds = %ins.shift.check
  %prevv = load %tag_Item, ptr %prevp, align 8
  %atj = getelementptr %tag_Item, ptr %data1, i64 %j3
  store %tag_Item %prevv, ptr %atj, align 8
  store i64 %j1, ptr %j, align 8
  br label %ins.shift.cond
}

define internal i64 @kelp_List__Item__median3(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at_a = getelementptr %tag_Item, ptr %data1, i64 %1
  %at_b = getelementptr %tag_Item, ptr %data1, i64 %2
  %at_c = getelementptr %tag_Item, ptr %data1, i64 %3
  %cab = call i32 @kelp_List__Item__cmp(ptr %at_a, ptr %at_b)
  %ab_lt = icmp slt i32 %cab, 0
  br i1 %ab_lt, label %m3.ablt, label %m3.age

m3.ablt:                                          ; preds = %entry
  %cbc = call i32 @kelp_List__Item__cmp(ptr %at_b, ptr %at_c)
  %bc_lt = icmp slt i32 %cbc, 0
  br i1 %bc_lt, label %m3.retb1, label %m3.chkac1

m3.age:                                           ; preds = %entry
  %cac2 = call i32 @kelp_List__Item__cmp(ptr %at_a, ptr %at_c)
  %ac_lt2 = icmp slt i32 %cac2, 0
  br i1 %ac_lt2, label %m3.reta2, label %m3.chkbc2

m3.retb1:                                         ; preds = %m3.ablt
  ret i64 %2

m3.chkac1:                                        ; preds = %m3.ablt
  %cac1 = call i32 @kelp_List__Item__cmp(ptr %at_a, ptr %at_c)
  %ac_lt1 = icmp slt i32 %cac1, 0
  br i1 %ac_lt1, label %m3.retc1, label %m3.reta1

m3.retc1:                                         ; preds = %m3.chkac1
  ret i64 %3

m3.reta1:                                         ; preds = %m3.chkac1
  ret i64 %1

m3.reta2:                                         ; preds = %m3.age
  ret i64 %1

m3.chkbc2:                                        ; preds = %m3.age
  %cbc2 = call i32 @kelp_List__Item__cmp(ptr %at_b, ptr %at_c)
  %bc_lt2 = icmp slt i32 %cbc2, 0
  br i1 %bc_lt2, label %m3.retc2, label %m3.retb2

m3.retc2:                                         ; preds = %m3.chkbc2
  ret i64 %3

m3.retb2:                                         ; preds = %m3.chkbc2
  ret i64 %2
}

define internal i64 @kelp_List__Item__partition(ptr %0, i64 %1, i64 %2) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %span = sub i64 %2, %1
  %half = udiv i64 %span, 2
  %mid = add i64 %1, %half
  %pividx = call i64 @kelp_List__Item__median3(ptr %0, i64 %1, i64 %mid, i64 %2)
  %pivptr = getelementptr %tag_Item, ptr %data1, i64 %pividx
  %pivot = alloca %tag_Item, align 8
  %pv = load %tag_Item, ptr %pivptr, align 8
  store %tag_Item %pv, ptr %pivot, align 8
  %l = alloca i64, align 8
  store i64 %1, ptr %l, align 8
  %rr = alloca i64, align 8
  %hi_p1 = add i64 %2, 1
  store i64 %hi_p1, ptr %rr, align 8
  %offsets_l = alloca [64 x i64], align 8
  %offsets_r = alloca [64 x i64], align 8
  %num_l = alloca i64, align 8
  store i64 0, ptr %num_l, align 8
  %num_r = alloca i64, align 8
  store i64 0, ptr %num_r, align 8
  %start_l = alloca i64, align 8
  store i64 0, ptr %start_l, align 8
  %start_r = alloca i64, align 8
  store i64 0, ptr %start_r, align 8
  br label %part.blk.cond

part.blk.cond:                                    ; preds = %part.after_adv_r, %entry
  %l2 = load i64, ptr %l, align 8
  %rr3 = load i64, ptr %rr, align 8
  %blk.width = sub i64 %rr3, %l2
  %has_room = icmp ugt i64 %blk.width, 128
  br i1 %has_room, label %part.blk.body, label %part.blk.done

part.blk.body:                                    ; preds = %part.blk.cond
  %rbase = sub i64 %rr3, 64
  %num_l0 = load i64, ptr %num_l, align 8
  %num_l0_zero = icmp eq i64 %num_l0, 0
  br i1 %num_l0_zero, label %part.fill_l, label %part.after_fill_l

part.blk.done:                                    ; preds = %part.blk.cond
  %l_final = load i64, ptr %l, align 8
  %rr_final = load i64, ptr %rr, align 8
  %i = alloca i64, align 8
  %l_final_m1 = sub i64 %l_final, 1
  store i64 %l_final_m1, ptr %i, align 8
  %j = alloca i64, align 8
  store i64 %rr_final, ptr %j, align 8
  br label %part.iscan

part.fill_l:                                      ; preds = %part.blk.body
  store i64 0, ptr %start_l, align 8
  store i64 0, ptr %num_l, align 8
  %fl_i = alloca i64, align 8
  store i64 0, ptr %fl_i, align 8
  br label %part.fl.cond

part.after_fill_l:                                ; preds = %part.fl.done, %part.blk.body
  %num_r0 = load i64, ptr %num_r, align 8
  %num_r0_zero = icmp eq i64 %num_r0, 0
  br i1 %num_r0_zero, label %part.fill_r, label %part.after_fill_r

part.fl.cond:                                     ; preds = %part.fl.body, %part.fill_l
  %fl_i.v = load i64, ptr %fl_i, align 8
  %fl_more = icmp ult i64 %fl_i.v, 64
  br i1 %fl_more, label %part.fl.body, label %part.fl.done

part.fl.body:                                     ; preds = %part.fl.cond
  %fl_numl = load i64, ptr %num_l, align 8
  %fl_off_ptr = getelementptr i64, ptr %offsets_l, i64 %fl_numl
  store i64 %fl_i.v, ptr %fl_off_ptr, align 8
  %fl_idx = add i64 %l2, %fl_i.v
  %fl_elem_ptr = getelementptr %tag_Item, ptr %data1, i64 %fl_idx
  %fl_cmp = call i32 @kelp_List__Item__cmp(ptr %fl_elem_ptr, ptr %pivot)
  %fl_wrong = icmp sge i32 %fl_cmp, 0
  %fl_wrong64 = zext i1 %fl_wrong to i64
  %fl_numl1 = add i64 %fl_numl, %fl_wrong64
  store i64 %fl_numl1, ptr %num_l, align 8
  %fl_i1 = add i64 %fl_i.v, 1
  store i64 %fl_i1, ptr %fl_i, align 8
  br label %part.fl.cond

part.fl.done:                                     ; preds = %part.fl.cond
  br label %part.after_fill_l

part.fill_r:                                      ; preds = %part.after_fill_l
  store i64 0, ptr %start_r, align 8
  store i64 0, ptr %num_r, align 8
  %fr_i = alloca i64, align 8
  store i64 0, ptr %fr_i, align 8
  br label %part.fr.cond

part.after_fill_r:                                ; preds = %part.fr.done, %part.after_fill_l
  %num_l1 = load i64, ptr %num_l, align 8
  %num_r1 = load i64, ptr %num_r, align 8
  %l_smaller = icmp ult i64 %num_l1, %num_r1
  %num = select i1 %l_smaller, i64 %num_l1, i64 %num_r1
  %start_l1 = load i64, ptr %start_l, align 8
  %start_r1 = load i64, ptr %start_r, align 8
  %sw_k = alloca i64, align 8
  store i64 0, ptr %sw_k, align 8
  br label %part.sw.cond

part.fr.cond:                                     ; preds = %part.fr.body, %part.fill_r
  %fr_i.v = load i64, ptr %fr_i, align 8
  %fr_more = icmp ult i64 %fr_i.v, 64
  br i1 %fr_more, label %part.fr.body, label %part.fr.done

part.fr.body:                                     ; preds = %part.fr.cond
  %fr_numr = load i64, ptr %num_r, align 8
  %fr_off_ptr = getelementptr i64, ptr %offsets_r, i64 %fr_numr
  store i64 %fr_i.v, ptr %fr_off_ptr, align 8
  %fr_idx = add i64 %rbase, %fr_i.v
  %fr_elem_ptr = getelementptr %tag_Item, ptr %data1, i64 %fr_idx
  %fr_cmp = call i32 @kelp_List__Item__cmp(ptr %fr_elem_ptr, ptr %pivot)
  %fr_wrong = icmp sle i32 %fr_cmp, 0
  %fr_wrong64 = zext i1 %fr_wrong to i64
  %fr_numr1 = add i64 %fr_numr, %fr_wrong64
  store i64 %fr_numr1, ptr %num_r, align 8
  %fr_i1 = add i64 %fr_i.v, 1
  store i64 %fr_i1, ptr %fr_i, align 8
  br label %part.fr.cond

part.fr.done:                                     ; preds = %part.fr.cond
  br label %part.after_fill_r

part.sw.cond:                                     ; preds = %part.sw.body, %part.after_fill_r
  %sw_k.v = load i64, ptr %sw_k, align 8
  %sw_more = icmp ult i64 %sw_k.v, %num
  br i1 %sw_more, label %part.sw.body, label %part.sw.done

part.sw.body:                                     ; preds = %part.sw.cond
  %sw_li_slot = add i64 %start_l1, %sw_k.v
  %sw_off_l_ptr = getelementptr i64, ptr %offsets_l, i64 %sw_li_slot
  %sw_off_l = load i64, ptr %sw_off_l_ptr, align 8
  %sw_li = add i64 %l2, %sw_off_l
  %sw_ri_slot = add i64 %start_r1, %sw_k.v
  %sw_off_r_ptr = getelementptr i64, ptr %offsets_r, i64 %sw_ri_slot
  %sw_off_r = load i64, ptr %sw_off_r_ptr, align 8
  %sw_ri = add i64 %rbase, %sw_off_r
  %sw_li_ptr = getelementptr %tag_Item, ptr %data1, i64 %sw_li
  %sw_ri_ptr = getelementptr %tag_Item, ptr %data1, i64 %sw_ri
  %sw_lv = load %tag_Item, ptr %sw_li_ptr, align 8
  %sw_rv = load %tag_Item, ptr %sw_ri_ptr, align 8
  store %tag_Item %sw_rv, ptr %sw_li_ptr, align 8
  store %tag_Item %sw_lv, ptr %sw_ri_ptr, align 8
  %sw_k1 = add i64 %sw_k.v, 1
  store i64 %sw_k1, ptr %sw_k, align 8
  br label %part.sw.cond

part.sw.done:                                     ; preds = %part.sw.cond
  %new_num_l = sub i64 %num_l1, %num
  %new_start_l = add i64 %start_l1, %num
  store i64 %new_num_l, ptr %num_l, align 8
  store i64 %new_start_l, ptr %start_l, align 8
  %new_num_r = sub i64 %num_r1, %num
  %new_start_r = add i64 %start_r1, %num
  store i64 %new_num_r, ptr %num_r, align 8
  store i64 %new_start_r, ptr %start_r, align 8
  %num_l_zero2 = icmp eq i64 %new_num_l, 0
  br i1 %num_l_zero2, label %part.adv_l, label %part.after_adv_l

part.adv_l:                                       ; preds = %part.sw.done
  %l_adv = add i64 %l2, 64
  store i64 %l_adv, ptr %l, align 8
  br label %part.after_adv_l

part.after_adv_l:                                 ; preds = %part.adv_l, %part.sw.done
  %num_r_zero2 = icmp eq i64 %new_num_r, 0
  br i1 %num_r_zero2, label %part.adv_r, label %part.after_adv_r

part.adv_r:                                       ; preds = %part.after_adv_l
  %rr_adv = sub i64 %rr3, 64
  store i64 %rr_adv, ptr %rr, align 8
  br label %part.after_adv_r

part.after_adv_r:                                 ; preds = %part.adv_r, %part.after_adv_l
  br label %part.blk.cond

part.iscan:                                       ; preds = %part.swap, %part.iscan, %part.blk.done
  %ii = load i64, ptr %i, align 8
  %ii1 = add i64 %ii, 1
  store i64 %ii1, ptr %i, align 8
  %at_i = getelementptr %tag_Item, ptr %data1, i64 %ii1
  %ci = call i32 @kelp_List__Item__cmp(ptr %at_i, ptr %pivot)
  %ilt = icmp slt i32 %ci, 0
  br i1 %ilt, label %part.iscan, label %part.jscan

part.jscan:                                       ; preds = %part.jscan, %part.iscan
  %jj = load i64, ptr %j, align 8
  %jj1 = sub i64 %jj, 1
  store i64 %jj1, ptr %j, align 8
  %at_j = getelementptr %tag_Item, ptr %data1, i64 %jj1
  %cj = call i32 @kelp_List__Item__cmp(ptr %at_j, ptr %pivot)
  %jgt = icmp sgt i32 %cj, 0
  br i1 %jgt, label %part.jscan, label %part.check

part.check:                                       ; preds = %part.jscan
  %ci2 = load i64, ptr %i, align 8
  %cj2 = load i64, ptr %j, align 8
  %ge = icmp uge i64 %ci2, %cj2
  br i1 %ge, label %part.ret, label %part.swap

part.ret:                                         ; preds = %part.check
  ret i64 %cj2

part.swap:                                        ; preds = %part.check
  %at_i2 = getelementptr %tag_Item, ptr %data1, i64 %ci2
  %at_j2 = getelementptr %tag_Item, ptr %data1, i64 %cj2
  %iv = load %tag_Item, ptr %at_i2, align 8
  %jv = load %tag_Item, ptr %at_j2, align 8
  store %tag_Item %jv, ptr %at_i2, align 8
  store %tag_Item %iv, ptr %at_j2, align 8
  br label %part.iscan
}

define internal void @kelp_List__Item__quicksort(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %lo = alloca i64, align 8
  store i64 %1, ptr %lo, align 8
  %hi = alloca i64, align 8
  store i64 %2, ptr %hi, align 8
  %depth = alloca i64, align 8
  store i64 %3, ptr %depth, align 8
  br label %qs.cond

qs.cond:                                          ; preds = %qs.right, %qs.left, %entry
  %lo1 = load i64, ptr %lo, align 8
  %hi2 = load i64, ptr %hi, align 8
  %width = sub i64 %hi2, %lo1
  %big = icmp ugt i64 %width, 32
  br i1 %big, label %qs.body, label %qs.done

qs.body:                                          ; preds = %qs.cond
  %depth3 = load i64, ptr %depth, align 8
  %depth_zero = icmp eq i64 %depth3, 0
  br i1 %depth_zero, label %qs.fallback, label %qs.partition

qs.done:                                          ; preds = %qs.cond
  %lo_f = load i64, ptr %lo, align 8
  %hi_f = load i64, ptr %hi, align 8
  call void @kelp_List__Item__insertion_sort(ptr %0, i64 %lo_f, i64 %hi_f)
  ret void

qs.fallback:                                      ; preds = %qs.body
  call void @kelp_List__Item__heapsort_range(ptr %0, i64 %lo1, i64 %hi2)
  ret void

qs.partition:                                     ; preds = %qs.body
  %depth1 = sub i64 %depth3, 1
  store i64 %depth1, ptr %depth, align 8
  %hi_incl = sub i64 %hi2, 1
  %p = call i64 @kelp_List__Item__partition(ptr %0, i64 %lo1, i64 %hi_incl)
  %p1 = add i64 %p, 1
  %left_len = sub i64 %p1, %lo1
  %right_len = sub i64 %hi2, %p1
  %left_smaller = icmp ult i64 %left_len, %right_len
  br i1 %left_smaller, label %qs.left, label %qs.right

qs.left:                                          ; preds = %qs.partition
  call void @kelp_List__Item__quicksort(ptr %0, i64 %lo1, i64 %p1, i64 %depth1)
  store i64 %p1, ptr %lo, align 8
  br label %qs.cond

qs.right:                                         ; preds = %qs.partition
  call void @kelp_List__Item__quicksort(ptr %0, i64 %p1, i64 %hi2, i64 %depth1)
  store i64 %p1, ptr %hi, align 8
  br label %qs.cond
}

define internal void @kelp_List__Item__sort(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %small = icmp ult i64 %len1, 2
  br i1 %small, label %sort.early, label %sort.cont

sort.early:                                       ; preds = %entry
  ret void

sort.cont:                                        ; preds = %entry
  %nn = alloca i64, align 8
  store i64 %len1, ptr %nn, align 8
  %depth = alloca i64, align 8
  store i64 0, ptr %depth, align 8
  br label %sort.log.cond

sort.log.cond:                                    ; preds = %sort.log.body, %sort.cont
  %nn2 = load i64, ptr %nn, align 8
  %more = icmp ugt i64 %nn2, 1
  br i1 %more, label %sort.log.body, label %sort.log.end

sort.log.body:                                    ; preds = %sort.log.cond
  %nn23 = lshr i64 %nn2, 1
  store i64 %nn23, ptr %nn, align 8
  %d = load i64, ptr %depth, align 8
  %d1 = add i64 %d, 1
  store i64 %d1, ptr %depth, align 8
  br label %sort.log.cond

sort.log.end:                                     ; preds = %sort.log.cond
  %dfinal = load i64, ptr %depth, align 8
  %depth_limit = mul i64 %dfinal, 2
  call void @kelp_List__Item__quicksort(ptr %0, i64 0, i64 %len1, i64 %depth_limit)
  ret void
}

define internal %kelp_List__Item @kelp_List__Item__slice(ptr %0, i64 %1, i64 %2) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %from_gt_to = icmp ugt i64 %1, %2
  %to_gt_len = icmp ugt i64 %2, %len1
  %oob = or i1 %from_gt_to, %to_gt_len
  br i1 %oob, label %oob2, label %slice

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.8)
  unreachable

slice:                                            ; preds = %entry
  %out = alloca %kelp_List__Item, align 8
  store %kelp_List__Item zeroinitializer, ptr %out, align 8
  %n = sub i64 %2, %1
  call void @kelp_List__Item__reserve(ptr %out, i64 %n)
  %i = alloca i64, align 8
  store i64 %1, ptr %i, align 8
  br label %slice.cond

slice.cond:                                       ; preds = %slice.body, %slice
  %i3 = load i64, ptr %i, align 8
  %more = icmp ult i64 %i3, %2
  br i1 %more, label %slice.body, label %slice.end

slice.body:                                       ; preds = %slice.cond
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %src = getelementptr %tag_Item, ptr %data4, i64 %i3
  %olen = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 1
  %olen5 = load i64, ptr %olen, align 8
  %odata = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 0
  %odata6 = load ptr, ptr %odata, align 8
  %dst = getelementptr %tag_Item, ptr %odata6, i64 %olen5
  call void @kelp_clone_Item(ptr %src, ptr %dst)
  %olen1 = add i64 %olen5, 1
  %len7 = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 1
  store i64 %olen1, ptr %len7, align 8
  %i1 = add i64 %i3, 1
  store i64 %i1, ptr %i, align 8
  br label %slice.cond

slice.end:                                        ; preds = %slice.cond
  %slice8 = load %kelp_List__Item, ptr %out, align 8
  ret %kelp_List__Item %slice8
}

define internal %kelp_List__Item @kelp_List__Item__clone(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %clone = call %kelp_List__Item @kelp_List__Item__slice(ptr %0, i64 0, i64 %len1)
  ret %kelp_List__Item %clone
}

define internal void @kelp_List__Item__insert(ptr %0, i64 %1, %tag_Item %2) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp ugt i64 %1, %len1
  br i1 %oob, label %oob2, label %insert

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.6)
  unreachable

insert:                                           ; preds = %entry
  %want = add i64 %len1, 1
  call void @kelp_List__Item__reserve(ptr %0, i64 %want)
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %j = alloca i64, align 8
  store i64 %len1, ptr %j, align 8
  br label %shift.cond

shift.cond:                                       ; preds = %shift.body, %insert
  %j4 = load i64, ptr %j, align 8
  %more = icmp ugt i64 %j4, %1
  br i1 %more, label %shift.body, label %shift.end

shift.body:                                       ; preds = %shift.cond
  %j1 = sub i64 %j4, 1
  %src = getelementptr %tag_Item, ptr %data3, i64 %j1
  %dst = getelementptr %tag_Item, ptr %data3, i64 %j4
  %sv = load %tag_Item, ptr %src, align 8
  store %tag_Item %sv, ptr %dst, align 8
  store i64 %j1, ptr %j, align 8
  br label %shift.cond

shift.end:                                        ; preds = %shift.cond
  %at = getelementptr %tag_Item, ptr %data3, i64 %1
  store %tag_Item %2, ptr %at, align 8
  %len15 = add i64 %len1, 1
  %len6 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  ret void
}

define internal %tag_Item @kelp_List__Item__remove(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %oob2, label %remove

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.7)
  unreachable

remove:                                           ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %out = alloca %tag_Item, align 8
  %at = getelementptr %tag_Item, ptr %data3, i64 %1
  %v = load %tag_Item, ptr %at, align 8
  store %tag_Item %v, ptr %out, align 8
  %j = alloca i64, align 8
  store i64 %1, ptr %j, align 8
  br label %shift.cond

shift.cond:                                       ; preds = %shift.body, %remove
  %j4 = load i64, ptr %j, align 8
  %j1 = add i64 %j4, 1
  %more = icmp ult i64 %j1, %len1
  br i1 %more, label %shift.body, label %shift.end

shift.body:                                       ; preds = %shift.cond
  %src = getelementptr %tag_Item, ptr %data3, i64 %j1
  %dst = getelementptr %tag_Item, ptr %data3, i64 %j4
  %sv = load %tag_Item, ptr %src, align 8
  store %tag_Item %sv, ptr %dst, align 8
  store i64 %j1, ptr %j, align 8
  br label %shift.cond

shift.end:                                        ; preds = %shift.cond
  %len15 = sub i64 %len1, 1
  %len6 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  %removed = load %tag_Item, ptr %out, align 8
  ret %tag_Item %removed
}

define internal void @kelp_list_drop__Item(ptr %0) {
entry:
  call void @kelp_List__Item__clear(ptr %0)
  %cap = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 2
  %cap1 = load i64, ptr %cap, align 8
  %bytes = mul i64 %cap1, 32
  %data = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  %data2 = load ptr, ptr %data, align 8
  %data3 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 0
  store ptr null, ptr %data3, align 8
  %cap4 = getelementptr inbounds nuw %kelp_List__Item, ptr %0, i32 0, i32 2
  store i64 0, ptr %cap4, align 8
  ret void
}

define internal %kelp_List__String @kelp_List__String__empty() {
entry:
  %l = alloca %kelp_List__String, align 8
  store %kelp_List__String zeroinitializer, ptr %l, align 8
  %ret = load %kelp_List__String, ptr %l, align 8
  ret %kelp_List__String %ret
}

define internal void @kelp_List__String__reserve(ptr %0, i64 %1) {
entry:
  %cap = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 2
  %cap1 = load i64, ptr %cap, align 8
  %enough = icmp ule i64 %1, %cap1
  br i1 %enough, label %ret, label %grow

ret:                                              ; preds = %alloc.ok, %entry
  ret void

grow:                                             ; preds = %entry
  %cap2 = alloca i64, align 8
  %zero = icmp eq i64 %cap1, 0
  %start = select i1 %zero, i64 4, i64 %cap1
  store i64 %start, ptr %cap2, align 8
  br label %grow.cond

grow.cond:                                        ; preds = %grow.body, %grow
  %cap3 = load i64, ptr %cap2, align 8
  %need = icmp ult i64 %cap3, %1
  br i1 %need, label %grow.body, label %grow.end

grow.body:                                        ; preds = %grow.cond
  %cap4 = load i64, ptr %cap2, align 8
  %cap25 = mul i64 %cap4, 2
  store i64 %cap25, ptr %cap2, align 8
  br label %grow.cond

grow.end:                                         ; preds = %grow.cond
  %cap6 = load i64, ptr %cap2, align 8
  %bytes = mul i64 %cap6, 24
  %old.bytes = mul i64 %cap1, 24
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data7 = load ptr, ptr %data, align 8
  %gc.arr = call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_String, i64 %cap6)
  %oom = icmp eq ptr %gc.arr, null
  br i1 %oom, label %oom8, label %alloc.ok

oom8:                                             ; preds = %grow.end
  call void @kelp_panic(ptr @panic.msg.9)
  unreachable

alloc.ok:                                         ; preds = %grow.end
  %2 = call ptr @memcpy(ptr %gc.arr, ptr %data7, i64 %old.bytes)
  %data9 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  store ptr %gc.arr, ptr %data9, align 8
  %cap10 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 2
  store i64 %cap6, ptr %cap10, align 8
  br label %ret
}

define internal %kelp_List__String @kelp_List__String__with_capacity(i64 %0) {
entry:
  %l = alloca %kelp_List__String, align 8
  store %kelp_List__String zeroinitializer, ptr %l, align 8
  call void @kelp_List__String__reserve(ptr %l, i64 %0)
  %wc = load %kelp_List__String, ptr %l, align 8
  ret %kelp_List__String %wc
}

define internal %kelp_List__String @kelp_List__String__of(%kelp_String %0) {
entry:
  %l = alloca %kelp_List__String, align 8
  store %kelp_List__String zeroinitializer, ptr %l, align 8
  call void @kelp_List__String__reserve(ptr %l, i64 1)
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %l, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data1, i64 0
  store %kelp_String %0, ptr %at, align 8
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %l, i32 0, i32 1
  store i64 1, ptr %len, align 8
  %of = load %kelp_List__String, ptr %l, align 8
  ret %kelp_List__String %of
}

define internal i64 @kelp_List__String__len(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  ret i64 %len1
}

define internal i1 @kelp_List__String__is_empty(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  ret i1 %empty
}

define internal void @kelp_List__String__push(ptr %0, %kelp_String %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %want = add i64 %len1, 1
  call void @kelp_List__String__reserve(ptr %0, i64 %want)
  %len2 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len3 = load i64, ptr %len2, align 8
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data4, i64 %len3
  store %kelp_String %1, ptr %at, align 8
  %len15 = add i64 %len3, 1
  %len6 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  ret void
}

define internal void @kelp_List__String__bounds(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %oob2, label %inbounds

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.10)
  unreachable

inbounds:                                         ; preds = %entry
  ret void
}

define internal ptr @kelp_List__String__get_const(ptr %0, i64 %1) {
entry:
  call void @kelp_List__String__bounds(ptr %0, i64 %1)
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data1, i64 %1
  ret ptr %at
}

define internal void @kelp_List__String__set(ptr %0, i64 %1, %kelp_String %2) {
entry:
  call void @kelp_List__String__bounds(ptr %0, i64 %1)
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data1, i64 %1
  store %kelp_String %2, ptr %at, align 8
  ret void
}

define internal %kelp_String @kelp_List__String__pop(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  br i1 %empty, label %empty2, label %pop

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.11)
  unreachable

pop:                                              ; preds = %entry
  %last = sub i64 %len1, 1
  %len3 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  store i64 %last, ptr %len3, align 8
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data4, i64 %last
  %pop5 = load %kelp_String, ptr %at, align 8
  ret %kelp_String %pop5
}

define internal void @kelp_List__String__clear(ptr %0) {
entry:
  %i = alloca i64, align 8
  store i64 0, ptr %i, align 8
  br label %clear.cond

clear.cond:                                       ; preds = %clear.body, %entry
  %i1 = load i64, ptr %i, align 8
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len2 = load i64, ptr %len, align 8
  %more = icmp ult i64 %i1, %len2
  br i1 %more, label %clear.body, label %clear.end

clear.body:                                       ; preds = %clear.cond
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data3, i64 %i1
  %i14 = add i64 %i1, 1
  store i64 %i14, ptr %i, align 8
  br label %clear.cond

clear.end:                                        ; preds = %clear.cond
  %len5 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  store i64 0, ptr %len5, align 8
  ret void
}

define internal void @kelp_List__String__reverse(ptr %0) {
entry:
  %i = alloca i64, align 8
  %j = alloca i64, align 8
  store i64 0, ptr %i, align 8
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  store i64 %len1, ptr %j, align 8
  br label %rev.cond

rev.cond:                                         ; preds = %rev.body, %entry
  %i2 = load i64, ptr %i, align 8
  %j3 = load i64, ptr %j, align 8
  %i1 = add i64 %i2, 1
  %more = icmp ult i64 %i1, %j3
  br i1 %more, label %rev.body, label %rev.end

rev.body:                                         ; preds = %rev.cond
  %j1 = sub i64 %j3, 1
  store i64 %j1, ptr %j, align 8
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %ai = getelementptr %kelp_String, ptr %data4, i64 %i2
  %aj = getelementptr %kelp_String, ptr %data4, i64 %j1
  %t = load %kelp_String, ptr %ai, align 8
  %u = load %kelp_String, ptr %aj, align 8
  store %kelp_String %u, ptr %ai, align 8
  store %kelp_String %t, ptr %aj, align 8
  %i15 = add i64 %i2, 1
  store i64 %i15, ptr %i, align 8
  br label %rev.cond

rev.end:                                          ; preds = %rev.cond
  ret void
}

define internal i32 @kelp_List__String__cmp(ptr %0, ptr %1) {
entry:
  %a = load %kelp_String, ptr %0, align 8
  %b = load %kelp_String, ptr %1, align 8
  %alen = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 1
  %alen1 = load i64, ptr %alen, align 8
  %blen = getelementptr inbounds nuw %kelp_String, ptr %1, i32 0, i32 1
  %blen2 = load i64, ptr %blen, align 8
  %lt = icmp ult i64 %alen1, %blen2
  %n = select i1 %lt, i64 %alen1, i64 %blen2
  %ad = getelementptr inbounds nuw %kelp_String, ptr %0, i32 0, i32 0
  %ad3 = load ptr, ptr %ad, align 8
  %bd = getelementptr inbounds nuw %kelp_String, ptr %1, i32 0, i32 0
  %bd4 = load ptr, ptr %bd, align 8
  %memcmp = call i32 @memcmp(ptr %ad3, ptr %bd4, i64 %n)
  %neg = icmp slt i32 %memcmp, 0
  %c1 = select i1 %neg, i32 -1, i32 1
  %npos = icmp ugt i64 %n, 0
  %cnz = icmp ne i32 %memcmp, 0
  %taken = and i1 %npos, %cnz
  %llt = icmp ult i64 %alen1, %blen2
  %lgt = icmp ugt i64 %alen1, %blen2
  %l0 = select i1 %lgt, i32 1, i32 0
  %lc = select i1 %llt, i32 -1, i32 %l0
  %cmp = select i1 %taken, i32 %c1, i32 %lc
  ret i32 %cmp
}

define internal void @kelp_List__String__sift_down(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %root = alloca i64, align 8
  store i64 %2, ptr %root, align 8
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  br label %sift.cond

sift.cond:                                        ; preds = %sift.swap, %entry
  %root2 = load i64, ptr %root, align 8
  %twice = mul i64 %root2, 2
  %child = add i64 %twice, 1
  %in_heap = icmp ult i64 %child, %3
  br i1 %in_heap, label %sift.pick, label %sift.end

sift.pick:                                        ; preds = %sift.cond
  %child1 = add i64 %child, 1
  %has_right = icmp ult i64 %child1, %3
  br i1 %has_right, label %sift.cmpchild, label %sift.afterpick

sift.compare:                                     ; preds = %sift.afterpick
  %root_abs = add i64 %1, %root2
  %chosen_abs = add i64 %1, %chosen
  %rootp = getelementptr %kelp_String, ptr %data1, i64 %root_abs
  %childp = getelementptr %kelp_String, ptr %data1, i64 %chosen_abs
  %cmp3 = call i32 @kelp_List__String__cmp(ptr %childp, ptr %rootp)
  %gt = icmp sgt i32 %cmp3, 0
  br i1 %gt, label %sift.swap, label %sift.end

sift.end:                                         ; preds = %sift.compare, %sift.cond
  ret void

sift.cmpchild:                                    ; preds = %sift.pick
  %child_abs = add i64 %1, %child
  %child1_abs = add i64 %1, %child1
  %l = getelementptr %kelp_String, ptr %data1, i64 %child_abs
  %r = getelementptr %kelp_String, ptr %data1, i64 %child1_abs
  %cmp = call i32 @kelp_List__String__cmp(ptr %l, ptr %r)
  %rb = icmp slt i32 %cmp, 0
  %bigger = select i1 %rb, i64 %child1, i64 %child
  br label %sift.afterpick

sift.afterpick:                                   ; preds = %sift.cmpchild, %sift.pick
  %chosen = phi i64 [ %child, %sift.pick ], [ %bigger, %sift.cmpchild ]
  br label %sift.compare

sift.swap:                                        ; preds = %sift.compare
  %pv = load %kelp_String, ptr %rootp, align 8
  %cv = load %kelp_String, ptr %childp, align 8
  store %kelp_String %cv, ptr %rootp, align 8
  store %kelp_String %pv, ptr %childp, align 8
  store i64 %chosen, ptr %root, align 8
  br label %sift.cond
}

define internal void @kelp_List__String__heapsort_range(ptr %0, i64 %1, i64 %2) {
entry:
  %n = sub i64 %2, %1
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %start = alloca i64, align 8
  %half = udiv i64 %n, 2
  store i64 %half, ptr %start, align 8
  br label %hr.heap.cond

hr.heap.cond:                                     ; preds = %hr.heap.body, %entry
  %s = load i64, ptr %start, align 8
  %spos = icmp ugt i64 %s, 0
  br i1 %spos, label %hr.heap.body, label %hr.heap.end

hr.heap.body:                                     ; preds = %hr.heap.cond
  %s1 = sub i64 %s, 1
  store i64 %s1, ptr %start, align 8
  call void @kelp_List__String__sift_down(ptr %0, i64 %1, i64 %s1, i64 %n)
  br label %hr.heap.cond

hr.heap.end:                                      ; preds = %hr.heap.cond
  %end = alloca i64, align 8
  store i64 %n, ptr %end, align 8
  br label %hr.sort.cond

hr.sort.cond:                                     ; preds = %hr.sort.body, %hr.heap.end
  %e = load i64, ptr %end, align 8
  %epos = icmp ugt i64 %e, 1
  br i1 %epos, label %hr.sort.body, label %hr.sort.end

hr.sort.body:                                     ; preds = %hr.sort.cond
  %e1 = sub i64 %e, 1
  store i64 %e1, ptr %end, align 8
  %lo_e1 = add i64 %1, %e1
  %d0 = getelementptr %kelp_String, ptr %data1, i64 %1
  %de = getelementptr %kelp_String, ptr %data1, i64 %lo_e1
  %tv = load %kelp_String, ptr %d0, align 8
  %ev = load %kelp_String, ptr %de, align 8
  store %kelp_String %ev, ptr %d0, align 8
  store %kelp_String %tv, ptr %de, align 8
  call void @kelp_List__String__sift_down(ptr %0, i64 %1, i64 0, i64 %e1)
  br label %hr.sort.cond

hr.sort.end:                                      ; preds = %hr.sort.cond
  ret void
}

define internal void @kelp_List__String__insertion_sort(ptr %0, i64 %1, i64 %2) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %i = alloca i64, align 8
  %lo1 = add i64 %1, 1
  store i64 %lo1, ptr %i, align 8
  br label %ins.cond

ins.cond:                                         ; preds = %ins.shift.end, %entry
  %i2 = load i64, ptr %i, align 8
  %more = icmp ult i64 %i2, %2
  br i1 %more, label %ins.body, label %ins.end

ins.body:                                         ; preds = %ins.cond
  %at_i = getelementptr %kelp_String, ptr %data1, i64 %i2
  %key = alloca %kelp_String, align 8
  %keyv = load %kelp_String, ptr %at_i, align 8
  store %kelp_String %keyv, ptr %key, align 8
  %j = alloca i64, align 8
  store i64 %i2, ptr %j, align 8
  br label %ins.shift.cond

ins.end:                                          ; preds = %ins.cond
  ret void

ins.shift.cond:                                   ; preds = %ins.shift.body, %ins.body
  %j3 = load i64, ptr %j, align 8
  %jgt = icmp ugt i64 %j3, %1
  br i1 %jgt, label %ins.shift.check, label %ins.shift.end

ins.shift.check:                                  ; preds = %ins.shift.cond
  %j1 = sub i64 %j3, 1
  %prevp = getelementptr %kelp_String, ptr %data1, i64 %j1
  %cmp = call i32 @kelp_List__String__cmp(ptr %prevp, ptr %key)
  %gtc = icmp sgt i32 %cmp, 0
  br i1 %gtc, label %ins.shift.body, label %ins.shift.end

ins.shift.end:                                    ; preds = %ins.shift.check, %ins.shift.cond
  %atj_final = getelementptr %kelp_String, ptr %data1, i64 %j3
  %keyv_final = load %kelp_String, ptr %key, align 8
  store %kelp_String %keyv_final, ptr %atj_final, align 8
  %i1 = add i64 %i2, 1
  store i64 %i1, ptr %i, align 8
  br label %ins.cond

ins.shift.body:                                   ; preds = %ins.shift.check
  %prevv = load %kelp_String, ptr %prevp, align 8
  %atj = getelementptr %kelp_String, ptr %data1, i64 %j3
  store %kelp_String %prevv, ptr %atj, align 8
  store i64 %j1, ptr %j, align 8
  br label %ins.shift.cond
}

define internal i64 @kelp_List__String__median3(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %at_a = getelementptr %kelp_String, ptr %data1, i64 %1
  %at_b = getelementptr %kelp_String, ptr %data1, i64 %2
  %at_c = getelementptr %kelp_String, ptr %data1, i64 %3
  %cab = call i32 @kelp_List__String__cmp(ptr %at_a, ptr %at_b)
  %ab_lt = icmp slt i32 %cab, 0
  br i1 %ab_lt, label %m3.ablt, label %m3.age

m3.ablt:                                          ; preds = %entry
  %cbc = call i32 @kelp_List__String__cmp(ptr %at_b, ptr %at_c)
  %bc_lt = icmp slt i32 %cbc, 0
  br i1 %bc_lt, label %m3.retb1, label %m3.chkac1

m3.age:                                           ; preds = %entry
  %cac2 = call i32 @kelp_List__String__cmp(ptr %at_a, ptr %at_c)
  %ac_lt2 = icmp slt i32 %cac2, 0
  br i1 %ac_lt2, label %m3.reta2, label %m3.chkbc2

m3.retb1:                                         ; preds = %m3.ablt
  ret i64 %2

m3.chkac1:                                        ; preds = %m3.ablt
  %cac1 = call i32 @kelp_List__String__cmp(ptr %at_a, ptr %at_c)
  %ac_lt1 = icmp slt i32 %cac1, 0
  br i1 %ac_lt1, label %m3.retc1, label %m3.reta1

m3.retc1:                                         ; preds = %m3.chkac1
  ret i64 %3

m3.reta1:                                         ; preds = %m3.chkac1
  ret i64 %1

m3.reta2:                                         ; preds = %m3.age
  ret i64 %1

m3.chkbc2:                                        ; preds = %m3.age
  %cbc2 = call i32 @kelp_List__String__cmp(ptr %at_b, ptr %at_c)
  %bc_lt2 = icmp slt i32 %cbc2, 0
  br i1 %bc_lt2, label %m3.retc2, label %m3.retb2

m3.retc2:                                         ; preds = %m3.chkbc2
  ret i64 %3

m3.retb2:                                         ; preds = %m3.chkbc2
  ret i64 %2
}

define internal i64 @kelp_List__String__partition(ptr %0, i64 %1, i64 %2) {
entry:
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data1 = load ptr, ptr %data, align 8
  %span = sub i64 %2, %1
  %half = udiv i64 %span, 2
  %mid = add i64 %1, %half
  %pividx = call i64 @kelp_List__String__median3(ptr %0, i64 %1, i64 %mid, i64 %2)
  %pivptr = getelementptr %kelp_String, ptr %data1, i64 %pividx
  %pivot = alloca %kelp_String, align 8
  %pv = load %kelp_String, ptr %pivptr, align 8
  store %kelp_String %pv, ptr %pivot, align 8
  %l = alloca i64, align 8
  store i64 %1, ptr %l, align 8
  %rr = alloca i64, align 8
  %hi_p1 = add i64 %2, 1
  store i64 %hi_p1, ptr %rr, align 8
  %offsets_l = alloca [64 x i64], align 8
  %offsets_r = alloca [64 x i64], align 8
  %num_l = alloca i64, align 8
  store i64 0, ptr %num_l, align 8
  %num_r = alloca i64, align 8
  store i64 0, ptr %num_r, align 8
  %start_l = alloca i64, align 8
  store i64 0, ptr %start_l, align 8
  %start_r = alloca i64, align 8
  store i64 0, ptr %start_r, align 8
  br label %part.blk.cond

part.blk.cond:                                    ; preds = %part.after_adv_r, %entry
  %l2 = load i64, ptr %l, align 8
  %rr3 = load i64, ptr %rr, align 8
  %blk.width = sub i64 %rr3, %l2
  %has_room = icmp ugt i64 %blk.width, 128
  br i1 %has_room, label %part.blk.body, label %part.blk.done

part.blk.body:                                    ; preds = %part.blk.cond
  %rbase = sub i64 %rr3, 64
  %num_l0 = load i64, ptr %num_l, align 8
  %num_l0_zero = icmp eq i64 %num_l0, 0
  br i1 %num_l0_zero, label %part.fill_l, label %part.after_fill_l

part.blk.done:                                    ; preds = %part.blk.cond
  %l_final = load i64, ptr %l, align 8
  %rr_final = load i64, ptr %rr, align 8
  %i = alloca i64, align 8
  %l_final_m1 = sub i64 %l_final, 1
  store i64 %l_final_m1, ptr %i, align 8
  %j = alloca i64, align 8
  store i64 %rr_final, ptr %j, align 8
  br label %part.iscan

part.fill_l:                                      ; preds = %part.blk.body
  store i64 0, ptr %start_l, align 8
  store i64 0, ptr %num_l, align 8
  %fl_i = alloca i64, align 8
  store i64 0, ptr %fl_i, align 8
  br label %part.fl.cond

part.after_fill_l:                                ; preds = %part.fl.done, %part.blk.body
  %num_r0 = load i64, ptr %num_r, align 8
  %num_r0_zero = icmp eq i64 %num_r0, 0
  br i1 %num_r0_zero, label %part.fill_r, label %part.after_fill_r

part.fl.cond:                                     ; preds = %part.fl.body, %part.fill_l
  %fl_i.v = load i64, ptr %fl_i, align 8
  %fl_more = icmp ult i64 %fl_i.v, 64
  br i1 %fl_more, label %part.fl.body, label %part.fl.done

part.fl.body:                                     ; preds = %part.fl.cond
  %fl_numl = load i64, ptr %num_l, align 8
  %fl_off_ptr = getelementptr i64, ptr %offsets_l, i64 %fl_numl
  store i64 %fl_i.v, ptr %fl_off_ptr, align 8
  %fl_idx = add i64 %l2, %fl_i.v
  %fl_elem_ptr = getelementptr %kelp_String, ptr %data1, i64 %fl_idx
  %fl_cmp = call i32 @kelp_List__String__cmp(ptr %fl_elem_ptr, ptr %pivot)
  %fl_wrong = icmp sge i32 %fl_cmp, 0
  %fl_wrong64 = zext i1 %fl_wrong to i64
  %fl_numl1 = add i64 %fl_numl, %fl_wrong64
  store i64 %fl_numl1, ptr %num_l, align 8
  %fl_i1 = add i64 %fl_i.v, 1
  store i64 %fl_i1, ptr %fl_i, align 8
  br label %part.fl.cond

part.fl.done:                                     ; preds = %part.fl.cond
  br label %part.after_fill_l

part.fill_r:                                      ; preds = %part.after_fill_l
  store i64 0, ptr %start_r, align 8
  store i64 0, ptr %num_r, align 8
  %fr_i = alloca i64, align 8
  store i64 0, ptr %fr_i, align 8
  br label %part.fr.cond

part.after_fill_r:                                ; preds = %part.fr.done, %part.after_fill_l
  %num_l1 = load i64, ptr %num_l, align 8
  %num_r1 = load i64, ptr %num_r, align 8
  %l_smaller = icmp ult i64 %num_l1, %num_r1
  %num = select i1 %l_smaller, i64 %num_l1, i64 %num_r1
  %start_l1 = load i64, ptr %start_l, align 8
  %start_r1 = load i64, ptr %start_r, align 8
  %sw_k = alloca i64, align 8
  store i64 0, ptr %sw_k, align 8
  br label %part.sw.cond

part.fr.cond:                                     ; preds = %part.fr.body, %part.fill_r
  %fr_i.v = load i64, ptr %fr_i, align 8
  %fr_more = icmp ult i64 %fr_i.v, 64
  br i1 %fr_more, label %part.fr.body, label %part.fr.done

part.fr.body:                                     ; preds = %part.fr.cond
  %fr_numr = load i64, ptr %num_r, align 8
  %fr_off_ptr = getelementptr i64, ptr %offsets_r, i64 %fr_numr
  store i64 %fr_i.v, ptr %fr_off_ptr, align 8
  %fr_idx = add i64 %rbase, %fr_i.v
  %fr_elem_ptr = getelementptr %kelp_String, ptr %data1, i64 %fr_idx
  %fr_cmp = call i32 @kelp_List__String__cmp(ptr %fr_elem_ptr, ptr %pivot)
  %fr_wrong = icmp sle i32 %fr_cmp, 0
  %fr_wrong64 = zext i1 %fr_wrong to i64
  %fr_numr1 = add i64 %fr_numr, %fr_wrong64
  store i64 %fr_numr1, ptr %num_r, align 8
  %fr_i1 = add i64 %fr_i.v, 1
  store i64 %fr_i1, ptr %fr_i, align 8
  br label %part.fr.cond

part.fr.done:                                     ; preds = %part.fr.cond
  br label %part.after_fill_r

part.sw.cond:                                     ; preds = %part.sw.body, %part.after_fill_r
  %sw_k.v = load i64, ptr %sw_k, align 8
  %sw_more = icmp ult i64 %sw_k.v, %num
  br i1 %sw_more, label %part.sw.body, label %part.sw.done

part.sw.body:                                     ; preds = %part.sw.cond
  %sw_li_slot = add i64 %start_l1, %sw_k.v
  %sw_off_l_ptr = getelementptr i64, ptr %offsets_l, i64 %sw_li_slot
  %sw_off_l = load i64, ptr %sw_off_l_ptr, align 8
  %sw_li = add i64 %l2, %sw_off_l
  %sw_ri_slot = add i64 %start_r1, %sw_k.v
  %sw_off_r_ptr = getelementptr i64, ptr %offsets_r, i64 %sw_ri_slot
  %sw_off_r = load i64, ptr %sw_off_r_ptr, align 8
  %sw_ri = add i64 %rbase, %sw_off_r
  %sw_li_ptr = getelementptr %kelp_String, ptr %data1, i64 %sw_li
  %sw_ri_ptr = getelementptr %kelp_String, ptr %data1, i64 %sw_ri
  %sw_lv = load %kelp_String, ptr %sw_li_ptr, align 8
  %sw_rv = load %kelp_String, ptr %sw_ri_ptr, align 8
  store %kelp_String %sw_rv, ptr %sw_li_ptr, align 8
  store %kelp_String %sw_lv, ptr %sw_ri_ptr, align 8
  %sw_k1 = add i64 %sw_k.v, 1
  store i64 %sw_k1, ptr %sw_k, align 8
  br label %part.sw.cond

part.sw.done:                                     ; preds = %part.sw.cond
  %new_num_l = sub i64 %num_l1, %num
  %new_start_l = add i64 %start_l1, %num
  store i64 %new_num_l, ptr %num_l, align 8
  store i64 %new_start_l, ptr %start_l, align 8
  %new_num_r = sub i64 %num_r1, %num
  %new_start_r = add i64 %start_r1, %num
  store i64 %new_num_r, ptr %num_r, align 8
  store i64 %new_start_r, ptr %start_r, align 8
  %num_l_zero2 = icmp eq i64 %new_num_l, 0
  br i1 %num_l_zero2, label %part.adv_l, label %part.after_adv_l

part.adv_l:                                       ; preds = %part.sw.done
  %l_adv = add i64 %l2, 64
  store i64 %l_adv, ptr %l, align 8
  br label %part.after_adv_l

part.after_adv_l:                                 ; preds = %part.adv_l, %part.sw.done
  %num_r_zero2 = icmp eq i64 %new_num_r, 0
  br i1 %num_r_zero2, label %part.adv_r, label %part.after_adv_r

part.adv_r:                                       ; preds = %part.after_adv_l
  %rr_adv = sub i64 %rr3, 64
  store i64 %rr_adv, ptr %rr, align 8
  br label %part.after_adv_r

part.after_adv_r:                                 ; preds = %part.adv_r, %part.after_adv_l
  br label %part.blk.cond

part.iscan:                                       ; preds = %part.swap, %part.iscan, %part.blk.done
  %ii = load i64, ptr %i, align 8
  %ii1 = add i64 %ii, 1
  store i64 %ii1, ptr %i, align 8
  %at_i = getelementptr %kelp_String, ptr %data1, i64 %ii1
  %ci = call i32 @kelp_List__String__cmp(ptr %at_i, ptr %pivot)
  %ilt = icmp slt i32 %ci, 0
  br i1 %ilt, label %part.iscan, label %part.jscan

part.jscan:                                       ; preds = %part.jscan, %part.iscan
  %jj = load i64, ptr %j, align 8
  %jj1 = sub i64 %jj, 1
  store i64 %jj1, ptr %j, align 8
  %at_j = getelementptr %kelp_String, ptr %data1, i64 %jj1
  %cj = call i32 @kelp_List__String__cmp(ptr %at_j, ptr %pivot)
  %jgt = icmp sgt i32 %cj, 0
  br i1 %jgt, label %part.jscan, label %part.check

part.check:                                       ; preds = %part.jscan
  %ci2 = load i64, ptr %i, align 8
  %cj2 = load i64, ptr %j, align 8
  %ge = icmp uge i64 %ci2, %cj2
  br i1 %ge, label %part.ret, label %part.swap

part.ret:                                         ; preds = %part.check
  ret i64 %cj2

part.swap:                                        ; preds = %part.check
  %at_i2 = getelementptr %kelp_String, ptr %data1, i64 %ci2
  %at_j2 = getelementptr %kelp_String, ptr %data1, i64 %cj2
  %iv = load %kelp_String, ptr %at_i2, align 8
  %jv = load %kelp_String, ptr %at_j2, align 8
  store %kelp_String %jv, ptr %at_i2, align 8
  store %kelp_String %iv, ptr %at_j2, align 8
  br label %part.iscan
}

define internal void @kelp_List__String__quicksort(ptr %0, i64 %1, i64 %2, i64 %3) {
entry:
  %lo = alloca i64, align 8
  store i64 %1, ptr %lo, align 8
  %hi = alloca i64, align 8
  store i64 %2, ptr %hi, align 8
  %depth = alloca i64, align 8
  store i64 %3, ptr %depth, align 8
  br label %qs.cond

qs.cond:                                          ; preds = %qs.right, %qs.left, %entry
  %lo1 = load i64, ptr %lo, align 8
  %hi2 = load i64, ptr %hi, align 8
  %width = sub i64 %hi2, %lo1
  %big = icmp ugt i64 %width, 32
  br i1 %big, label %qs.body, label %qs.done

qs.body:                                          ; preds = %qs.cond
  %depth3 = load i64, ptr %depth, align 8
  %depth_zero = icmp eq i64 %depth3, 0
  br i1 %depth_zero, label %qs.fallback, label %qs.partition

qs.done:                                          ; preds = %qs.cond
  %lo_f = load i64, ptr %lo, align 8
  %hi_f = load i64, ptr %hi, align 8
  call void @kelp_List__String__insertion_sort(ptr %0, i64 %lo_f, i64 %hi_f)
  ret void

qs.fallback:                                      ; preds = %qs.body
  call void @kelp_List__String__heapsort_range(ptr %0, i64 %lo1, i64 %hi2)
  ret void

qs.partition:                                     ; preds = %qs.body
  %depth1 = sub i64 %depth3, 1
  store i64 %depth1, ptr %depth, align 8
  %hi_incl = sub i64 %hi2, 1
  %p = call i64 @kelp_List__String__partition(ptr %0, i64 %lo1, i64 %hi_incl)
  %p1 = add i64 %p, 1
  %left_len = sub i64 %p1, %lo1
  %right_len = sub i64 %hi2, %p1
  %left_smaller = icmp ult i64 %left_len, %right_len
  br i1 %left_smaller, label %qs.left, label %qs.right

qs.left:                                          ; preds = %qs.partition
  call void @kelp_List__String__quicksort(ptr %0, i64 %lo1, i64 %p1, i64 %depth1)
  store i64 %p1, ptr %lo, align 8
  br label %qs.cond

qs.right:                                         ; preds = %qs.partition
  call void @kelp_List__String__quicksort(ptr %0, i64 %p1, i64 %hi2, i64 %depth1)
  store i64 %p1, ptr %hi, align 8
  br label %qs.cond
}

define internal void @kelp_List__String__sort(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %small = icmp ult i64 %len1, 2
  br i1 %small, label %sort.early, label %sort.cont

sort.early:                                       ; preds = %entry
  ret void

sort.cont:                                        ; preds = %entry
  %nn = alloca i64, align 8
  store i64 %len1, ptr %nn, align 8
  %depth = alloca i64, align 8
  store i64 0, ptr %depth, align 8
  br label %sort.log.cond

sort.log.cond:                                    ; preds = %sort.log.body, %sort.cont
  %nn2 = load i64, ptr %nn, align 8
  %more = icmp ugt i64 %nn2, 1
  br i1 %more, label %sort.log.body, label %sort.log.end

sort.log.body:                                    ; preds = %sort.log.cond
  %nn23 = lshr i64 %nn2, 1
  store i64 %nn23, ptr %nn, align 8
  %d = load i64, ptr %depth, align 8
  %d1 = add i64 %d, 1
  store i64 %d1, ptr %depth, align 8
  br label %sort.log.cond

sort.log.end:                                     ; preds = %sort.log.cond
  %dfinal = load i64, ptr %depth, align 8
  %depth_limit = mul i64 %dfinal, 2
  call void @kelp_List__String__quicksort(ptr %0, i64 0, i64 %len1, i64 %depth_limit)
  ret void
}

define internal i1 @kelp_List__String__contains(ptr %0, ptr %1) {
entry:
  %found = alloca i1, align 1
  %i = alloca i64, align 8
  store i1 false, ptr %found, align 1
  store i64 0, ptr %i, align 8
  br label %has.cond

has.cond:                                         ; preds = %has.next, %entry
  %i1 = load i64, ptr %i, align 8
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len2 = load i64, ptr %len, align 8
  %more = icmp ult i64 %i1, %len2
  br i1 %more, label %has.body, label %has.end

has.body:                                         ; preds = %has.cond
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %at = getelementptr %kelp_String, ptr %data3, i64 %i1
  %alen = getelementptr inbounds nuw %kelp_String, ptr %at, i32 0, i32 1
  %alen4 = load i64, ptr %alen, align 8
  %vlen = getelementptr inbounds nuw %kelp_String, ptr %1, i32 0, i32 1
  %vlen5 = load i64, ptr %vlen, align 8
  %leq = icmp eq i64 %alen4, %vlen5
  %z = icmp eq i64 %vlen5, 0
  %zleq = and i1 %z, %leq
  br i1 %z, label %has.after, label %has.cmp

has.end:                                          ; preds = %has.found, %has.cond
  %found9 = load i1, ptr %found, align 1
  ret i1 %found9

has.cmp:                                          ; preds = %has.body
  %ad = getelementptr inbounds nuw %kelp_String, ptr %at, i32 0, i32 0
  %ad6 = load ptr, ptr %ad, align 8
  %vd = getelementptr inbounds nuw %kelp_String, ptr %1, i32 0, i32 0
  %vd7 = load ptr, ptr %vd, align 8
  %memcmp = call i32 @memcmp(ptr %ad6, ptr %vd7, i64 %vlen5)
  %c0 = icmp eq i32 %memcmp, 0
  %ceq = and i1 %leq, %c0
  br label %has.after

has.after:                                        ; preds = %has.cmp, %has.body
  %hit = phi i1 [ %zleq, %has.body ], [ %ceq, %has.cmp ]
  br i1 %hit, label %has.found, label %has.next

has.found:                                        ; preds = %has.after
  store i1 true, ptr %found, align 1
  br label %has.end

has.next:                                         ; preds = %has.after
  %i18 = add i64 %i1, 1
  store i64 %i18, ptr %i, align 8
  br label %has.cond
}

define internal %kelp_List__String @kelp_List__String__slice(ptr %0, i64 %1, i64 %2) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %from_gt_to = icmp ugt i64 %1, %2
  %to_gt_len = icmp ugt i64 %2, %len1
  %oob = or i1 %from_gt_to, %to_gt_len
  br i1 %oob, label %oob2, label %slice

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.14)
  unreachable

slice:                                            ; preds = %entry
  %out = alloca %kelp_List__String, align 8
  store %kelp_List__String zeroinitializer, ptr %out, align 8
  %n = sub i64 %2, %1
  call void @kelp_List__String__reserve(ptr %out, i64 %n)
  %i = alloca i64, align 8
  store i64 %1, ptr %i, align 8
  br label %slice.cond

slice.cond:                                       ; preds = %slice.body, %slice
  %i3 = load i64, ptr %i, align 8
  %more = icmp ult i64 %i3, %2
  br i1 %more, label %slice.body, label %slice.end

slice.body:                                       ; preds = %slice.cond
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data4 = load ptr, ptr %data, align 8
  %src = getelementptr %kelp_String, ptr %data4, i64 %i3
  %olen = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 1
  %olen5 = load i64, ptr %olen, align 8
  %odata = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 0
  %odata6 = load ptr, ptr %odata, align 8
  %dst = getelementptr %kelp_String, ptr %odata6, i64 %olen5
  call void @kelp_String__clone(ptr %dst, ptr %src)
  %olen1 = add i64 %olen5, 1
  %len7 = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 1
  store i64 %olen1, ptr %len7, align 8
  %i1 = add i64 %i3, 1
  store i64 %i1, ptr %i, align 8
  br label %slice.cond

slice.end:                                        ; preds = %slice.cond
  %slice8 = load %kelp_List__String, ptr %out, align 8
  ret %kelp_List__String %slice8
}

define internal %kelp_List__String @kelp_List__String__clone(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %clone = call %kelp_List__String @kelp_List__String__slice(ptr %0, i64 0, i64 %len1)
  ret %kelp_List__String %clone
}

define internal void @kelp_List__String__insert(ptr %0, i64 %1, %kelp_String %2) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp ugt i64 %1, %len1
  br i1 %oob, label %oob2, label %insert

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.12)
  unreachable

insert:                                           ; preds = %entry
  %want = add i64 %len1, 1
  call void @kelp_List__String__reserve(ptr %0, i64 %want)
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %j = alloca i64, align 8
  store i64 %len1, ptr %j, align 8
  br label %shift.cond

shift.cond:                                       ; preds = %shift.body, %insert
  %j4 = load i64, ptr %j, align 8
  %more = icmp ugt i64 %j4, %1
  br i1 %more, label %shift.body, label %shift.end

shift.body:                                       ; preds = %shift.cond
  %j1 = sub i64 %j4, 1
  %src = getelementptr %kelp_String, ptr %data3, i64 %j1
  %dst = getelementptr %kelp_String, ptr %data3, i64 %j4
  %sv = load %kelp_String, ptr %src, align 8
  store %kelp_String %sv, ptr %dst, align 8
  store i64 %j1, ptr %j, align 8
  br label %shift.cond

shift.end:                                        ; preds = %shift.cond
  %at = getelementptr %kelp_String, ptr %data3, i64 %1
  store %kelp_String %2, ptr %at, align 8
  %len15 = add i64 %len1, 1
  %len6 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  ret void
}

define internal %kelp_String @kelp_List__String__remove(ptr %0, i64 %1) {
entry:
  %len = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  %len1 = load i64, ptr %len, align 8
  %oob = icmp uge i64 %1, %len1
  br i1 %oob, label %oob2, label %remove

oob2:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.13)
  unreachable

remove:                                           ; preds = %entry
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data3 = load ptr, ptr %data, align 8
  %out = alloca %kelp_String, align 8
  %at = getelementptr %kelp_String, ptr %data3, i64 %1
  %v = load %kelp_String, ptr %at, align 8
  store %kelp_String %v, ptr %out, align 8
  %j = alloca i64, align 8
  store i64 %1, ptr %j, align 8
  br label %shift.cond

shift.cond:                                       ; preds = %shift.body, %remove
  %j4 = load i64, ptr %j, align 8
  %j1 = add i64 %j4, 1
  %more = icmp ult i64 %j1, %len1
  br i1 %more, label %shift.body, label %shift.end

shift.body:                                       ; preds = %shift.cond
  %src = getelementptr %kelp_String, ptr %data3, i64 %j1
  %dst = getelementptr %kelp_String, ptr %data3, i64 %j4
  %sv = load %kelp_String, ptr %src, align 8
  store %kelp_String %sv, ptr %dst, align 8
  store i64 %j1, ptr %j, align 8
  br label %shift.cond

shift.end:                                        ; preds = %shift.cond
  %len15 = sub i64 %len1, 1
  %len6 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 1
  store i64 %len15, ptr %len6, align 8
  %removed = load %kelp_String, ptr %out, align 8
  ret %kelp_String %removed
}

define internal void @kelp_list_drop__String(ptr %0) {
entry:
  call void @kelp_List__String__clear(ptr %0)
  %cap = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 2
  %cap1 = load i64, ptr %cap, align 8
  %bytes = mul i64 %cap1, 24
  %data = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  %data2 = load ptr, ptr %data, align 8
  %data3 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 0
  store ptr null, ptr %data3, align 8
  %cap4 = getelementptr inbounds nuw %kelp_List__String, ptr %0, i32 0, i32 2
  store i64 0, ptr %cap4, align 8
  ret void
}

define internal %kelp_Box__i32 @kelp_Box__i32__empty() {
entry:
  %b = alloca %kelp_Box__i32, align 8
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %b, i32 0, i32 0
  store ptr null, ptr %value, align 8
  %empty = load %kelp_Box__i32, ptr %b, align 8
  ret %kelp_Box__i32 %empty
}

define internal %kelp_Box__i32 @kelp_Box__i32__of(i32 %0) {
entry:
  %b = alloca %kelp_Box__i32, align 8
  %gc = call ptr @kelp_gc_alloc(ptr @kelp_gc_type_i32)
  %oom = icmp eq ptr %gc, null
  br i1 %oom, label %oom1, label %alloc.ok

oom1:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.15)
  unreachable

alloc.ok:                                         ; preds = %entry
  store i32 %0, ptr %gc, align 4
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %b, i32 0, i32 0
  store ptr %gc, ptr %value, align 8
  %of = load %kelp_Box__i32, ptr %b, align 8
  ret %kelp_Box__i32 %of
}

define internal i1 @kelp_Box__i32__is_empty(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  ret i1 %empty
}

define internal i32 @kelp_Box__i32__get(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %empty2, label %get

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.16)
  unreachable

get:                                              ; preds = %entry
  %get3 = load i32, ptr %value1, align 4
  ret i32 %get3
}

define internal void @kelp_Box__i32__set(ptr %0, i32 %1) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %alloc, label %have

alloc:                                            ; preds = %entry
  %gc = call ptr @kelp_gc_alloc(ptr @kelp_gc_type_i32)
  %oom = icmp eq ptr %gc, null
  br i1 %oom, label %oom2, label %alloc.ok

have:                                             ; preds = %entry
  br label %store

store:                                            ; preds = %have, %alloc.ok
  %value3 = load ptr, ptr %value, align 8
  store i32 %1, ptr %value3, align 4
  ret void

oom2:                                             ; preds = %alloc
  call void @kelp_panic(ptr @panic.msg.17)
  unreachable

alloc.ok:                                         ; preds = %alloc
  store ptr %gc, ptr %value, align 8
  br label %store
}

define internal i32 @kelp_Box__i32__take(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %empty2, label %take

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.18)
  unreachable

take:                                             ; preds = %entry
  %take3 = load i32, ptr %value1, align 4
  store ptr null, ptr %value, align 8
  ret i32 %take3
}

define internal void @kelp_box_drop__i32(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__i32, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %ret, label %live

ret:                                              ; preds = %live, %entry
  ret void

live:                                             ; preds = %entry
  store ptr null, ptr %value, align 8
  br label %ret
}

define internal %kelp_Map__String__Item @kelp_Map__String__Item__new() {
entry:
  %m = alloca %kelp_Map__String__Item, align 8
  store %kelp_Map__String__Item zeroinitializer, ptr %m, align 8
  %new = load %kelp_Map__String__Item, ptr %m, align 8
  ret %kelp_Map__String__Item %new
}

define internal %kelp_Map__String__Item @kelp_Map__String__Item__with_capacity(i64 %0) {
entry:
  %m = alloca %kelp_Map__String__Item, align 8
  store %kelp_Map__String__Item zeroinitializer, ptr %m, align 8
  %zerocap = icmp eq i64 %0, 0
  br i1 %zerocap, label %wc.empty, label %wc.sized

wc.empty:                                         ; preds = %alloc.ok, %entry
  %map = load %kelp_Map__String__Item, ptr %m, align 8
  ret %kelp_Map__String__Item %map

wc.sized:                                         ; preds = %entry
  %bytes = mul i64 %0, 64
  %gc.arr = call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_map_entry.kelp_Map__String__Item, i64 %0)
  %oom = icmp eq ptr %gc.arr, null
  br i1 %oom, label %oom1, label %alloc.ok

oom1:                                             ; preds = %wc.sized
  call void @kelp_panic(ptr @panic.msg.19)
  unreachable

alloc.ok:                                         ; preds = %wc.sized
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %m, i32 0, i32 0
  store ptr %gc.arr, ptr %entries, align 8
  %entry_cap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %m, i32 0, i32 2
  store i64 %0, ptr %entry_cap, align 8
  call void @kelp_Map__String__Item__rehash(ptr %m, i64 %0)
  br label %wc.empty
}

define internal i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %2) {
entry:
  store i1 false, ptr %2, align 1
  %icap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  %icap1 = load i64, ptr %icap, align 8
  %noindex = icmp eq i64 %icap1, 0
  br i1 %noindex, label %find.none, label %find.cont

find.none:                                        ; preds = %entry
  ret i64 0

find.cont:                                        ; preds = %entry
  %mask = sub i64 %icap1, 1
  %len = call i64 @strlen(ptr %1)
  %maphash.h = alloca i64, align 8
  store i64 1469598103934665603, ptr %maphash.h, align 8
  %maphash.idx = alloca i64, align 8
  store i64 0, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.cond:                                     ; preds = %maphash.body, %find.cont
  %maphash.i = load i64, ptr %maphash.idx, align 8
  %maphash.more = icmp ult i64 %maphash.i, %len
  br i1 %maphash.more, label %maphash.body, label %maphash.after

maphash.body:                                     ; preds = %maphash.cond
  %maphash.cptr = getelementptr i8, ptr %1, i64 %maphash.i
  %maphash.c = load i8, ptr %maphash.cptr, align 1
  %maphash.cw = zext i8 %maphash.c to i64
  %maphash.h1 = load i64, ptr %maphash.h, align 8
  %maphash.hx = xor i64 %maphash.h1, %maphash.cw
  %maphash.hm = mul i64 %maphash.hx, 1099511628211
  store i64 %maphash.hm, ptr %maphash.h, align 8
  %maphash.next = add i64 %maphash.i, 1
  store i64 %maphash.next, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.after:                                    ; preds = %maphash.cond
  %maphash.result = load i64, ptr %maphash.h, align 8
  %i0 = and i64 %maphash.result, %mask
  %i = alloca i64, align 8
  %tomb = alloca i64, align 8
  store i64 %i0, ptr %i, align 8
  store i64 -1, ptr %tomb, align 8
  br label %find.loop

find.loop:                                        ; preds = %find.next, %maphash.after
  %i2 = load i64, ptr %i, align 8
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index3 = load ptr, ptr %index, align 8
  %slot = getelementptr i32, ptr %index3, i64 %i2
  %slot4 = load i32, ptr %slot, align 4
  %iszero = icmp eq i32 %slot4, 0
  br i1 %iszero, label %find.empty, label %find.probe

find.empty:                                       ; preds = %find.loop
  %tomb5 = load i64, ptr %tomb, align 8
  %istomb = icmp eq i64 %tomb5, -1
  %slot6 = select i1 %istomb, i64 %i2, i64 %tomb5
  ret i64 %slot6

find.probe:                                       ; preds = %find.loop
  %istomb7 = icmp eq i32 %slot4, -1
  br i1 %istomb7, label %find.tomb, label %find.live

find.tomb:                                        ; preds = %find.probe
  %tomb8 = load i64, ptr %tomb, align 8
  %istomb9 = icmp eq i64 %tomb8, -1
  %tomb10 = select i1 %istomb9, i64 %i2, i64 %tomb8
  store i64 %tomb10, ptr %tomb, align 8
  br label %find.next

find.live:                                        ; preds = %find.probe
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries11 = load ptr, ptr %entries, align 8
  %slot64 = zext i32 %slot4 to i64
  %e = sub i64 %slot64, 1
  %entry12 = getelementptr %kelp_Map__String__Item_entry, ptr %entries11, i64 %e
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry12, i32 0, i32 2
  %live13 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live13, 0
  br i1 %dead, label %find.next, label %find.eq

find.eq:                                          ; preds = %find.live
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry12, i32 0, i32 0
  %len14 = call i64 @strlen(ptr %1)
  %eq = call i1 @kelp_map_key_eq(ptr %key, ptr %1, i64 %len14)
  br i1 %eq, label %find.found, label %find.next

find.found:                                       ; preds = %find.eq
  store i1 true, ptr %2, align 1
  ret i64 %i2

find.next:                                        ; preds = %find.eq, %find.live, %find.tomb
  %i1 = add i64 %i2, 1
  %i215 = and i64 %i1, %mask
  store i64 %i215, ptr %i, align 8
  br label %find.loop
}

define internal void @kelp_Map__String__Item__rehash(ptr %0, i64 %1) {
entry:
  %cap = alloca i64, align 8
  store i64 8, ptr %cap, align 8
  br label %rehash.cond

rehash.cond:                                      ; preds = %rehash.body, %entry
  %cap1 = load i64, ptr %cap, align 8
  %want4 = mul i64 %1, 4
  %need = icmp ult i64 %cap1, %want4
  br i1 %need, label %rehash.body, label %rehash.end

rehash.body:                                      ; preds = %rehash.cond
  %cap2 = mul i64 %cap1, 2
  store i64 %cap2, ptr %cap, align 8
  br label %rehash.cond

rehash.end:                                       ; preds = %rehash.cond
  %cap3 = load i64, ptr %cap, align 8
  %icap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  %icap4 = load i64, ptr %icap, align 8
  %old.bytes = mul i64 %icap4, 4
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index5 = load ptr, ptr %index, align 8
  %bytes = mul i64 %cap3, 4
  %gc.arr = call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_u32, i64 %cap3)
  %oom = icmp eq ptr %gc.arr, null
  br i1 %oom, label %oom6, label %alloc.ok

oom6:                                             ; preds = %rehash.end
  call void @kelp_panic(ptr @panic.msg.20)
  unreachable

alloc.ok:                                         ; preds = %rehash.end
  %2 = call ptr @memset(ptr %gc.arr, i32 0, i64 %bytes)
  %index7 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  store ptr %gc.arr, ptr %index7, align 8
  %index_cap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  store i64 %cap3, ptr %index_cap, align 8
  %index_used = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  store i64 0, ptr %index_used, align 8
  %e = alloca i64, align 8
  store i64 0, ptr %e, align 8
  br label %rehash.walk

rehash.walk:                                      ; preds = %rehash.skip, %alloc.ok
  %e8 = load i64, ptr %e, align 8
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count9 = load i64, ptr %count, align 8
  %more = icmp ult i64 %e8, %count9
  br i1 %more, label %rehash.walk.body, label %rehash.walk.end

rehash.walk.body:                                 ; preds = %rehash.walk
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries10 = load ptr, ptr %entries, align 8
  %entry11 = getelementptr %kelp_Map__String__Item_entry, ptr %entries10, i64 %e8
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry11, i32 0, i32 2
  %live12 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live12, 0
  br i1 %dead, label %rehash.skip, label %rehash.place

rehash.walk.end:                                  ; preds = %rehash.walk
  ret void

rehash.skip:                                      ; preds = %rehash.probe.end, %rehash.walk.body
  %e1 = add i64 %e8, 1
  store i64 %e1, ptr %e, align 8
  br label %rehash.walk

rehash.place:                                     ; preds = %rehash.walk.body
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry11, i32 0, i32 0
  %d = getelementptr inbounds nuw %kelp_String, ptr %key, i32 0, i32 0
  %d13 = load ptr, ptr %d, align 8
  %n = getelementptr inbounds nuw %kelp_String, ptr %key, i32 0, i32 1
  %n14 = load i64, ptr %n, align 8
  %maphash.h = alloca i64, align 8
  store i64 1469598103934665603, ptr %maphash.h, align 8
  %maphash.idx = alloca i64, align 8
  store i64 0, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.cond:                                     ; preds = %maphash.body, %rehash.place
  %maphash.i = load i64, ptr %maphash.idx, align 8
  %maphash.more = icmp ult i64 %maphash.i, %n14
  br i1 %maphash.more, label %maphash.body, label %maphash.after

maphash.body:                                     ; preds = %maphash.cond
  %maphash.cptr = getelementptr i8, ptr %d13, i64 %maphash.i
  %maphash.c = load i8, ptr %maphash.cptr, align 1
  %maphash.cw = zext i8 %maphash.c to i64
  %maphash.h1 = load i64, ptr %maphash.h, align 8
  %maphash.hx = xor i64 %maphash.h1, %maphash.cw
  %maphash.hm = mul i64 %maphash.hx, 1099511628211
  store i64 %maphash.hm, ptr %maphash.h, align 8
  %maphash.next = add i64 %maphash.i, 1
  store i64 %maphash.next, ptr %maphash.idx, align 8
  br label %maphash.cond

maphash.after:                                    ; preds = %maphash.cond
  %maphash.result = load i64, ptr %maphash.h, align 8
  %mask = sub i64 %cap3, 1
  %slot = alloca i64, align 8
  %j0 = and i64 %maphash.result, %mask
  store i64 %j0, ptr %slot, align 8
  br label %rehash.probe

rehash.probe:                                     ; preds = %rehash.probe.body, %maphash.after
  %j = load i64, ptr %slot, align 8
  %index15 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index16 = load ptr, ptr %index15, align 8
  %at = getelementptr i32, ptr %index16, i64 %j
  %holder = load i32, ptr %at, align 4
  %occupied = icmp ne i32 %holder, 0
  br i1 %occupied, label %rehash.probe.body, label %rehash.probe.end

rehash.probe.body:                                ; preds = %rehash.probe
  %j1 = add i64 %j, 1
  %jm = and i64 %j1, %mask
  store i64 %jm, ptr %slot, align 8
  br label %rehash.probe

rehash.probe.end:                                 ; preds = %rehash.probe
  %j17 = load i64, ptr %slot, align 8
  %index18 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index19 = load ptr, ptr %index18, align 8
  %at20 = getelementptr i32, ptr %index19, i64 %j17
  %ep1 = add i64 %e8, 1
  %ep1_32 = trunc i64 %ep1 to i32
  store i32 %ep1_32, ptr %at20, align 4
  %used = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  %used21 = load i64, ptr %used, align 8
  %used1 = add i64 %used21, 1
  %used22 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  store i64 %used1, ptr %used22, align 8
  br label %rehash.skip
}

define internal void @kelp_Map__String__Item__insert(ptr %0, ptr %1, %tag_Item %2) {
entry:
  %key.tmp = alloca %kelp_String, align 8
  %icap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  %icap1 = load i64, ptr %icap, align 8
  %iused = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  %iused2 = load i64, ptr %iused, align 8
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len3 = load i64, ptr %len, align 8
  %need = add i64 %iused2, 1
  %need4 = mul i64 %need, 4
  %cap3 = mul i64 %icap1, 3
  %crowded = icmp uge i64 %need4, %cap3
  %noindex = icmp eq i64 %icap1, 0
  %grow = or i1 %crowded, %noindex
  br i1 %grow, label %insert.rehash, label %insert.placed

insert.rehash:                                    ; preds = %entry
  %want = add i64 %len3, 1
  call void @kelp_Map__String__Item__rehash(ptr %0, i64 %want)
  br label %insert.placed

insert.placed:                                    ; preds = %insert.rehash, %entry
  %found = alloca i1, align 1
  %slot = call i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %found)
  %found4 = load i1, ptr %found, align 1
  br i1 %found4, label %insert.replace, label %insert.fresh

insert.replace:                                   ; preds = %insert.placed
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index5 = load ptr, ptr %index, align 8
  %at = getelementptr i32, ptr %index5, i64 %slot
  %holder = load i32, ptr %at, align 4
  %holder64 = zext i32 %holder to i64
  %e = sub i64 %holder64, 1
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries6 = load ptr, ptr %entries, align 8
  %entry7 = getelementptr %kelp_Map__String__Item_entry, ptr %entries6, i64 %e
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry7, i32 0, i32 1
  store %tag_Item %2, ptr %value, align 8
  ret void

insert.fresh:                                     ; preds = %insert.placed
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count8 = load i64, ptr %count, align 8
  %ecap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 2
  %ecap9 = load i64, ptr %ecap, align 8
  %full = icmp eq i64 %count8, %ecap9
  br i1 %full, label %insert.resize, label %insert.space

insert.resize:                                    ; preds = %insert.fresh
  %zero = icmp eq i64 %ecap9, 0
  %double = mul i64 %ecap9, 2
  %newcap = select i1 %zero, i64 8, i64 %double
  %entries10 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries11 = load ptr, ptr %entries10, align 8
  %bytes = mul i64 %newcap, 64
  %old.bytes = mul i64 %ecap9, 64
  %gc.arr = call ptr @kelp_gc_alloc_array(ptr @kelp_gc_type_map_entry.kelp_Map__String__Item, i64 %newcap)
  %oom = icmp eq ptr %gc.arr, null
  br i1 %oom, label %oom12, label %alloc.ok

insert.space:                                     ; preds = %alloc.ok, %insert.fresh
  %count14 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count15 = load i64, ptr %count14, align 8
  %e1 = add i64 %count15, 1
  %entry_count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  store i64 %e1, ptr %entry_count, align 8
  %entries16 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries17 = load ptr, ptr %entries16, align 8
  %entry18 = getelementptr %kelp_Map__String__Item_entry, ptr %entries17, i64 %count15
  call void @kelp_String__from_str(ptr %key.tmp, ptr %1)
  %key = load %kelp_String, ptr %key.tmp, align 8
  %key19 = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry18, i32 0, i32 0
  store %kelp_String %key, ptr %key19, align 8
  %value20 = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry18, i32 0, i32 1
  store %tag_Item %2, ptr %value20, align 8
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry18, i32 0, i32 2
  store i8 1, ptr %live, align 1
  %index21 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index22 = load ptr, ptr %index21, align 8
  %at23 = getelementptr i32, ptr %index22, i64 %slot
  %holder24 = load i32, ptr %at23, align 4
  %wasempty = icmp eq i32 %holder24, 0
  br i1 %wasempty, label %insert.bump, label %insert.done

oom12:                                            ; preds = %insert.resize
  call void @kelp_panic(ptr @panic.msg.23)
  unreachable

alloc.ok:                                         ; preds = %insert.resize
  %3 = call ptr @memcpy(ptr %gc.arr, ptr %entries11, i64 %old.bytes)
  %entries13 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  store ptr %gc.arr, ptr %entries13, align 8
  %entry_cap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 2
  store i64 %newcap, ptr %entry_cap, align 8
  br label %insert.space

insert.bump:                                      ; preds = %insert.space
  %used = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  %used25 = load i64, ptr %used, align 8
  %used1 = add i64 %used25, 1
  %index_used = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  store i64 %used1, ptr %index_used, align 8
  br label %insert.done

insert.done:                                      ; preds = %insert.bump, %insert.space
  %index26 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index27 = load ptr, ptr %index26, align 8
  %at28 = getelementptr i32, ptr %index27, i64 %slot
  %ep1 = add i64 %count15, 1
  %ep1_32 = trunc i64 %ep1 to i32
  store i32 %ep1_32, ptr %at28, align 4
  %len29 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len30 = load i64, ptr %len29, align 8
  %len1 = add i64 %len30, 1
  %len31 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  store i64 %len1, ptr %len31, align 8
  ret void
}

define internal i1 @kelp_Map__String__Item__has(ptr %0, ptr %1) {
entry:
  %found = alloca i1, align 1
  %2 = call i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %found)
  %has = load i1, ptr %found, align 1
  ret i1 %has
}

define internal ptr @kelp_Map__String__Item__get_const(ptr %0, ptr %1) {
entry:
  %found = alloca i1, align 1
  %slot = call i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %found)
  %found1 = load i1, ptr %found, align 1
  %missing = icmp eq i1 %found1, false
  br i1 %missing, label %missing2, label %get

missing2:                                         ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.21)
  unreachable

get:                                              ; preds = %entry
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index3 = load ptr, ptr %index, align 8
  %at = getelementptr i32, ptr %index3, i64 %slot
  %holder = load i32, ptr %at, align 4
  %holder64 = zext i32 %holder to i64
  %e = sub i64 %holder64, 1
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries4 = load ptr, ptr %entries, align 8
  %entry5 = getelementptr %kelp_Map__String__Item_entry, ptr %entries4, i64 %e
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 1
  ret ptr %value
}

define internal %tag_Item @kelp_Map__String__Item__get_or(ptr %0, ptr %1, %tag_Item %2) {
entry:
  %found = alloca i1, align 1
  %clone = alloca %tag_Item, align 8
  %slot = call i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %found)
  %found1 = load i1, ptr %found, align 1
  %missing = icmp eq i1 %found1, false
  %fallback = alloca %tag_Item, align 8
  store %tag_Item %2, ptr %fallback, align 8
  br i1 %missing, label %missing2, label %hit

missing2:                                         ; preds = %entry
  %fallback3 = load %tag_Item, ptr %fallback, align 8
  ret %tag_Item %fallback3

hit:                                              ; preds = %entry
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index4 = load ptr, ptr %index, align 8
  %at = getelementptr i32, ptr %index4, i64 %slot
  %holder = load i32, ptr %at, align 4
  %holder64 = zext i32 %holder to i64
  %e = sub i64 %holder64, 1
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries5 = load ptr, ptr %entries, align 8
  %entry6 = getelementptr %kelp_Map__String__Item_entry, ptr %entries5, i64 %e
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry6, i32 0, i32 1
  call void @kelp_clone_Item(ptr %value, ptr %clone)
  %clone7 = load %tag_Item, ptr %clone, align 8
  ret %tag_Item %clone7
}

define internal %tag_Item @kelp_Map__String__Item__remove(ptr %0, ptr %1) {
entry:
  %found = alloca i1, align 1
  %slot = call i64 @kelp_Map__String__Item__find(ptr %0, ptr %1, ptr %found)
  %found1 = load i1, ptr %found, align 1
  %missing = icmp eq i1 %found1, false
  br i1 %missing, label %missing2, label %remove

missing2:                                         ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.22)
  unreachable

remove:                                           ; preds = %entry
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index3 = load ptr, ptr %index, align 8
  %at = getelementptr i32, ptr %index3, i64 %slot
  %holder = load i32, ptr %at, align 4
  %holder64 = zext i32 %holder to i64
  %e = sub i64 %holder64, 1
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries4 = load ptr, ptr %entries, align 8
  %entry5 = getelementptr %kelp_Map__String__Item_entry, ptr %entries4, i64 %e
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 1
  %out = alloca %tag_Item, align 8
  %v = load %tag_Item, ptr %value, align 8
  store %tag_Item %v, ptr %out, align 8
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 0
  call void @kelp_string_drop(ptr %key)
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 2
  store i8 0, ptr %live, align 1
  %index6 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index7 = load ptr, ptr %index6, align 8
  %at8 = getelementptr i32, ptr %index7, i64 %slot
  store i32 -1, ptr %at8, align 4
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len9 = load i64, ptr %len, align 8
  %len1 = sub i64 %len9, 1
  %len10 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  store i64 %len1, ptr %len10, align 8
  %removed = load %tag_Item, ptr %out, align 8
  ret %tag_Item %removed
}

define internal void @kelp_Map__String__Item__clear(ptr %0) {
entry:
  %e = alloca i64, align 8
  store i64 0, ptr %e, align 8
  br label %clear.cond

clear.cond:                                       ; preds = %clear.skip, %entry
  %e1 = load i64, ptr %e, align 8
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count2 = load i64, ptr %count, align 8
  %more = icmp ult i64 %e1, %count2
  br i1 %more, label %clear.body, label %clear.end

clear.body:                                       ; preds = %clear.cond
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries3 = load ptr, ptr %entries, align 8
  %entry4 = getelementptr %kelp_Map__String__Item_entry, ptr %entries3, i64 %e1
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 2
  %live5 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live5, 0
  br i1 %dead, label %clear.skip, label %clear.free

clear.end:                                        ; preds = %clear.cond
  %entry_count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  store i64 0, ptr %entry_count, align 8
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  store i64 0, ptr %len, align 8
  %index_used = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 6
  store i64 0, ptr %index_used, align 8
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index8 = load ptr, ptr %index, align 8
  %noindex = icmp eq ptr %index8, null
  br i1 %noindex, label %clear.ret, label %clear.index

clear.skip:                                       ; preds = %clear.free, %clear.body
  %e17 = add i64 %e1, 1
  store i64 %e17, ptr %e, align 8
  br label %clear.cond

clear.free:                                       ; preds = %clear.body
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 1
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 0
  call void @kelp_string_drop(ptr %key)
  %live6 = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 2
  store i8 0, ptr %live6, align 1
  br label %clear.skip

clear.index:                                      ; preds = %clear.end
  %icap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  %icap9 = load i64, ptr %icap, align 8
  %bytes = mul i64 %icap9, 4
  %1 = call ptr @memset(ptr %index8, i32 0, i64 %bytes)
  br label %clear.ret

clear.ret:                                        ; preds = %clear.index, %clear.end
  ret void
}

define internal i64 @kelp_Map__String__Item__len(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len1 = load i64, ptr %len, align 8
  ret i64 %len1
}

define internal i1 @kelp_Map__String__Item__is_empty(ptr %0) {
entry:
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len1 = load i64, ptr %len, align 8
  %empty = icmp eq i64 %len1, 0
  ret i1 %empty
}

define internal %kelp_List__String @kelp_Map__String__Item__keys(ptr %0) {
entry:
  %out = alloca %kelp_List__String, align 8
  %key.tmp = alloca %kelp_String, align 8
  store %kelp_List__String zeroinitializer, ptr %out, align 8
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len1 = load i64, ptr %len, align 8
  call void @kelp_List__String__reserve(ptr %out, i64 %len1)
  %e = alloca i64, align 8
  store i64 0, ptr %e, align 8
  br label %keys.cond

keys.cond:                                        ; preds = %keys.skip, %entry
  %e2 = load i64, ptr %e, align 8
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count3 = load i64, ptr %count, align 8
  %more = icmp ult i64 %e2, %count3
  br i1 %more, label %keys.body, label %keys.end

keys.body:                                        ; preds = %keys.cond
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries4 = load ptr, ptr %entries, align 8
  %entry5 = getelementptr %kelp_Map__String__Item_entry, ptr %entries4, i64 %e2
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 2
  %live6 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live6, 0
  br i1 %dead, label %keys.skip, label %keys.append

keys.end:                                         ; preds = %keys.cond
  %keys = load %kelp_List__String, ptr %out, align 8
  ret %kelp_List__String %keys

keys.skip:                                        ; preds = %keys.append, %keys.body
  %e1 = add i64 %e2, 1
  store i64 %e1, ptr %e, align 8
  br label %keys.cond

keys.append:                                      ; preds = %keys.body
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 0
  call void @kelp_String__clone(ptr %key.tmp, ptr %key)
  %key7 = load %kelp_String, ptr %key.tmp, align 8
  %olen = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 1
  %olen8 = load i64, ptr %olen, align 8
  %odata = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 0
  %odata9 = load ptr, ptr %odata, align 8
  %dst = getelementptr %kelp_String, ptr %odata9, i64 %olen8
  store %kelp_String %key7, ptr %dst, align 8
  %olen1 = add i64 %olen8, 1
  %len10 = getelementptr inbounds nuw %kelp_List__String, ptr %out, i32 0, i32 1
  store i64 %olen1, ptr %len10, align 8
  br label %keys.skip
}

define internal %kelp_List__Item @kelp_Map__String__Item__values(ptr %0) {
entry:
  %out = alloca %kelp_List__Item, align 8
  %value.tmp = alloca %tag_Item, align 8
  store %kelp_List__Item zeroinitializer, ptr %out, align 8
  %len = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 3
  %len1 = load i64, ptr %len, align 8
  call void @kelp_List__Item__reserve(ptr %out, i64 %len1)
  %e = alloca i64, align 8
  store i64 0, ptr %e, align 8
  br label %values.cond

values.cond:                                      ; preds = %values.skip, %entry
  %e2 = load i64, ptr %e, align 8
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count3 = load i64, ptr %count, align 8
  %more = icmp ult i64 %e2, %count3
  br i1 %more, label %values.body, label %values.end

values.body:                                      ; preds = %values.cond
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries4 = load ptr, ptr %entries, align 8
  %entry5 = getelementptr %kelp_Map__String__Item_entry, ptr %entries4, i64 %e2
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 2
  %live6 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live6, 0
  br i1 %dead, label %values.skip, label %values.append

values.end:                                       ; preds = %values.cond
  %values = load %kelp_List__Item, ptr %out, align 8
  ret %kelp_List__Item %values

values.skip:                                      ; preds = %values.append, %values.body
  %e1 = add i64 %e2, 1
  store i64 %e1, ptr %e, align 8
  br label %values.cond

values.append:                                    ; preds = %values.body
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry5, i32 0, i32 1
  call void @kelp_clone_Item(ptr %value, ptr %value.tmp)
  %clone = load %tag_Item, ptr %value.tmp, align 8
  %olen = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 1
  %olen7 = load i64, ptr %olen, align 8
  %odata = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 0
  %odata8 = load ptr, ptr %odata, align 8
  %dst = getelementptr %tag_Item, ptr %odata8, i64 %olen7
  store %tag_Item %clone, ptr %dst, align 8
  %olen1 = add i64 %olen7, 1
  %len9 = getelementptr inbounds nuw %kelp_List__Item, ptr %out, i32 0, i32 1
  store i64 %olen1, ptr %len9, align 8
  br label %values.skip
}

define internal void @kelp_map_drop__String__Item(ptr %0) {
entry:
  %e = alloca i64, align 8
  store i64 0, ptr %e, align 8
  br label %drop.cond

drop.cond:                                        ; preds = %drop.skip, %entry
  %e1 = load i64, ptr %e, align 8
  %count = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 1
  %count2 = load i64, ptr %count, align 8
  %more = icmp ult i64 %e1, %count2
  br i1 %more, label %drop.body, label %drop.end

drop.body:                                        ; preds = %drop.cond
  %entries = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries3 = load ptr, ptr %entries, align 8
  %entry4 = getelementptr %kelp_Map__String__Item_entry, ptr %entries3, i64 %e1
  %live = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 2
  %live5 = load i8, ptr %live, align 1
  %dead = icmp eq i8 %live5, 0
  br i1 %dead, label %drop.skip, label %drop.free

drop.end:                                         ; preds = %drop.cond
  %ecap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 2
  %ecap7 = load i64, ptr %ecap, align 8
  %entry.bytes = mul i64 %ecap7, 64
  %icap = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 5
  %icap8 = load i64, ptr %icap, align 8
  %index.bytes = mul i64 %icap8, 4
  %entries9 = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 0
  %entries10 = load ptr, ptr %entries9, align 8
  %index = getelementptr inbounds nuw %kelp_Map__String__Item, ptr %0, i32 0, i32 4
  %index11 = load ptr, ptr %index, align 8
  store %kelp_Map__String__Item zeroinitializer, ptr %0, align 8
  ret void

drop.skip:                                        ; preds = %drop.free, %drop.body
  %e16 = add i64 %e1, 1
  store i64 %e16, ptr %e, align 8
  br label %drop.cond

drop.free:                                        ; preds = %drop.body
  %value = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 1
  %key = getelementptr inbounds nuw %kelp_Map__String__Item_entry, ptr %entry4, i32 0, i32 0
  call void @kelp_string_drop(ptr %key)
  br label %drop.skip
}

define void @kelp_closure_drop__Closure__i32__i32(ptr %0) {
entry:
  %env = getelementptr inbounds nuw %kelp_Closure__i32__i32, ptr %0, i32 0, i32 1
  %env1 = load ptr, ptr %env, align 8
  %no.env = icmp eq ptr %env1, null
  br i1 %no.env, label %exit, label %drop

exit:                                             ; preds = %drop, %entry
  ret void

drop:                                             ; preds = %entry
  %drop2 = getelementptr inbounds nuw %kelp_Closure__i32__i32, ptr %0, i32 0, i32 2
  %drop3 = load ptr, ptr %drop2, align 8
  call void %drop3(ptr %env1)
  store ptr null, ptr %env, align 8
  br label %exit
}

define void @kelp_env_drop____funlit1(ptr %0) {
entry:
  %c0 = getelementptr inbounds nuw %kelp_Env____funlit1, ptr %0, i32 0, i32 0
  ret void
}

define i32 @main(i32 %0, ptr %1) !dbg !3 {
entry:
  %sret.out3 = alloca %kelp_String, align 8, !dbg !6
  %sret.out = alloca %kelp_String, align 8, !dbg !6
  %names = alloca %kelp_List__String, align 8, !dbg !6
  %items = alloca %kelp_Map__String__Item, align 8, !dbg !6
  %b = alloca %kelp_Box__i32, align 8, !dbg !6
  %add = alloca %kelp_Closure__i32__i32, align 8, !dbg !6
  call void @kelp_gc_init(i64 8388608, i64 268435456), !dbg !6
  call void @kelp_gc_thread_attach(), !dbg !6
  store i32 %0, ptr @kelp_argc, align 4, !dbg !6
  store ptr %1, ptr @kelp_argv, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !7
  %call = call %kelp_List__String @kelp_List__String__empty(), !dbg !7
  store %kelp_List__String %call, ptr %names, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !8
  call void @kelp_gc_safepoint(), !dbg !8
  call void @kelp_String__from_str(ptr %sret.out, ptr @str), !dbg !6
  %sret.val = load %kelp_String, ptr %sret.out, align 8, !dbg !6
  call void @kelp_List__String__push(ptr %names, %kelp_String %sret.val), !dbg !6
  call void @kelp_gc_safepoint(), !dbg !9
  call void @kelp_gc_safepoint(), !dbg !9
  %call1 = call i64 @kelp_List__String__len(ptr %names), !dbg !9
  %2 = call i32 (ptr, ...) @printf(ptr @str.24, i64 %call1), !dbg !9
  call void @kelp_gc_safepoint(), !dbg !10
  %call2 = call %kelp_Map__String__Item @kelp_Map__String__Item__new(), !dbg !10
  store %kelp_Map__String__Item %call2, ptr %items, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !11
  call void @kelp_gc_safepoint(), !dbg !11
  call void @kelp_String__from_str(ptr %sret.out3, ptr @str.26), !dbg !6
  %sret.val4 = load %kelp_String, ptr %sret.out3, align 8, !dbg !6
  %name = insertvalue %tag_Item undef, %kelp_String %sret.val4, 0, !dbg !6
  %n = insertvalue %tag_Item %name, i32 1, 1, !dbg !6
  call void @kelp_Map__String__Item__insert(ptr %items, ptr @str.25, %tag_Item %n), !dbg !6
  call void @kelp_gc_safepoint(), !dbg !12
  %call5 = call %kelp_Box__i32 @kelp_Box__i32__of(i32 41), !dbg !12
  store %kelp_Box__i32 %call5, ptr %b, align 8, !dbg !6
  %env = call ptr @kelp_gc_alloc(ptr @"kelp_gc_type_env.#funlit1"), !dbg !13
  %oom = icmp eq ptr %env, null, !dbg !13
  br i1 %oom, label %oom6, label %alloc.ok, !dbg !13

oom6:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.27), !dbg !13
  unreachable, !dbg !13

alloc.ok:                                         ; preds = %entry
  %b7 = load %kelp_Box__i32, ptr %b, align 8, !dbg !13
  %c0 = getelementptr inbounds nuw %kelp_Env____funlit1, ptr %env, i32 0, i32 0, !dbg !13
  store %kelp_Box__i32 %b7, ptr %c0, align 8, !dbg !13
  %env8 = insertvalue %kelp_Closure__i32__i32 { ptr @kelp___funlit1, ptr undef, ptr undef }, ptr %env, 1, !dbg !13
  %drop = insertvalue %kelp_Closure__i32__i32 %env8, ptr @kelp_env_drop____funlit1, 2, !dbg !13
  store %kelp_Closure__i32__i32 %drop, ptr %add, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !14
  %fn = getelementptr inbounds nuw %kelp_Closure__i32__i32, ptr %add, i32 0, i32 0, !dbg !14
  %fn9 = load ptr, ptr %fn, align 8, !dbg !14
  %env10 = getelementptr inbounds nuw %kelp_Closure__i32__i32, ptr %add, i32 0, i32 1, !dbg !14
  %env11 = load ptr, ptr %env10, align 8, !dbg !14
  %call12 = call i32 %fn9(ptr %env11, i32 1), !dbg !14
  %3 = call i32 (ptr, ...) @printf(ptr @str.28, i32 %call12), !dbg !14
  call void @kelp_closure_drop__Closure__i32__i32(ptr %add), !dbg !14
  ret i32 0, !dbg !14
}

define i32 @kelp___funlit1(ptr %0, i32 %1) !dbg !15 {
entry:
  %x = alloca i32, align 4, !dbg !16
  %env = alloca ptr, align 8, !dbg !16
  store ptr %0, ptr %env, align 8, !dbg !16
  store i32 %1, ptr %x, align 4, !dbg !16
  %x1 = load i32, ptr %x, align 4, !dbg !17
  call void @kelp_gc_safepoint(), !dbg !17
  %env2 = load ptr, ptr %env, align 8, !dbg !17
  %c0 = getelementptr inbounds nuw %kelp_Env____funlit1, ptr %env2, i32 0, i32 0, !dbg !17
  %call = call i32 @kelp_Box__i32__get(ptr %c0), !dbg !17
  %add = add i32 %x1, %call, !dbg !17
  ret i32 %add, !dbg !17
}

!llvm.dbg.cu = !{!0, !0}
!llvm.module.flags = !{!2}

!0 = distinct !DICompileUnit(language: DW_LANG_C, file: !1, producer: "kelpc 26.10.5", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, splitDebugInlining: false)
!1 = !DIFile(filename: "containers.kp", directory: "@FIXTURE_DIR@")
!2 = !{i32 2, !"Debug Info Version", i32 3}
!3 = distinct !DISubprogram(name: "main", linkageName: "main", scope: !1, file: !1, line: 8, type: !4, scopeLine: 8, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !0)
!4 = !DISubroutineType(flags: DIFlagPrototyped, types: !5)
!5 = !{}
!6 = !DILocation(line: 8, column: 1, scope: !3)
!7 = !DILocation(line: 9, column: 31, scope: !3)
!8 = !DILocation(line: 10, column: 5, scope: !3)
!9 = !DILocation(line: 11, column: 5, scope: !3)
!10 = !DILocation(line: 13, column: 36, scope: !3)
!11 = !DILocation(line: 14, column: 5, scope: !3)
!12 = !DILocation(line: 16, column: 13, scope: !3)
!13 = !DILocation(line: 17, column: 15, scope: !3)
!14 = !DILocation(line: 18, column: 5, scope: !3)
!15 = distinct !DISubprogram(name: "#funlit1", linkageName: "kelp___funlit1", scope: !1, file: !1, line: 17, type: !4, scopeLine: 17, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !0)
!16 = !DILocation(line: 17, column: 15, scope: !15)
!17 = !DILocation(line: 17, column: 41, scope: !15)
