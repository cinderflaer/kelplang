; ModuleID = 'kelp'
source_filename = "kelp"
target datalayout = "e-m:e-p270:32:32-p271:32:32-p272:64:64-i64:64-i128:128-f80:128-n8:16:32:64-S128"
target triple = "x86_64-unknown-linux-gnu"

%kelp_gc_type = type { i32, i32, i32, i32, i32, ptr, i32, i32, ptr }
%kelp_Bytes = type { ptr, i64, i64 }
%kelp_String = type { ptr, i64, i64 }
%kelp_View = type { ptr, i64 }
%kelp_Box__Node = type { ptr }
%tag_Node = type { %kelp_Box__Node, %kelp_Box__Node }

@kelp_argc = external global i32
@kelp_argv = external global ptr
@kelp_caps_node_env = external global ptr
@kelp_caps_node_frames = external global ptr
@kelp_caps_node_fs = external global ptr
@kelp_caps_node_irq = external global ptr
@kelp_caps_node_net = external global ptr
@kelp_caps_node_process = external global ptr
@kelp_caps_node_rand = external global ptr
@kelp_caps_node_time = external global ptr
@kelp_caps_node_ui = external global ptr
@panic.msg = private unnamed_addr constant [31 x i8] c"Bytes.get: index out of bounds\00", align 1
@panic.msg.1 = private unnamed_addr constant [31 x i8] c"Bytes.set: index out of bounds\00", align 1
@panic.msg.2 = private unnamed_addr constant [30 x i8] c"View.get: index out of bounds\00", align 1
@kelp_gc_ptrs_Node = internal constant [2 x i32] [i32 0, i32 8]
@kelp_gc_type_Node = internal constant %kelp_gc_type { i32 1, i32 16, i32 8, i32 0, i32 2, ptr @kelp_gc_ptrs_Node, i32 0, i32 0, ptr null }
@panic.msg.3 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.4 = private unnamed_addr constant [26 x i8] c"Box.get: the box is empty\00", align 1
@panic.msg.5 = private unnamed_addr constant [14 x i8] c"out of memory\00", align 1
@panic.msg.6 = private unnamed_addr constant [27 x i8] c"Box.take: the box is empty\00", align 1
@str = private unnamed_addr constant [5 x i8] c"true\00", align 1
@str.7 = private unnamed_addr constant [6 x i8] c"false\00", align 1
@str.8 = private unnamed_addr constant [4 x i8] c"%s\0A\00", align 1

declare i32 @printf(ptr, ...)

declare void @kelp_panic(ptr)

declare void @kelp_assert(i1 zeroext)

declare ptr @kelp_thread_new(i64, i64)

declare void @kelp_thread_start(ptr, ptr, ptr, ptr, ptr)

declare void @kelp_thread_start_rooted(ptr, ptr, ptr, ptr, ptr, ptr, ptr)

declare i32 @kelp_thread_wait(ptr)

declare ptr @kelp_thread_arg(ptr)

declare ptr @kelp_thread_result(ptr)

declare ptr @kelp_thread_message(ptr)

declare void @kelp_thread_dispose(ptr)

declare void @kelp_thread_drop(ptr)

declare ptr @kelp_mutex_new(ptr, i64, ptr)

declare ptr @kelp_mutex_new_rooted(ptr, i64, ptr, ptr)

declare void @kelp_mutex_run(ptr, ptr, ptr, ptr, ptr)

declare void @kelp_mutex_into(ptr, ptr)

declare void @kelp_mutex_drop(ptr)

declare ptr @kelp_mutex_share(ptr)

declare void @kelp_mutex_swap(ptr, ptr, ptr)

declare ptr @kelp_condvar_share(ptr)

declare ptr @kelp_rwlock_new(ptr, i64, ptr)

declare ptr @kelp_rwlock_new_rooted(ptr, i64, ptr, ptr)

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

declare ptr @kelp_box_pool()

declare ptr @kelp_box_alloc(i64, i64)

declare void @kelp_box_free(ptr, i64, i64)

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

declare void @kelp_gc_init(i64, i64, i64, i64, i64, i64)

declare void @kelp_gc_safepoint()

declare void @kelp_gc_thread_attach()

declare void @kelp_gc_thread_detach()

define internal %kelp_Box__Node @kelp_Box__Node__empty() {
entry:
  %b = alloca %kelp_Box__Node, align 8
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %b, i32 0, i32 0
  store ptr null, ptr %value, align 8
  %empty = load %kelp_Box__Node, ptr %b, align 8
  ret %kelp_Box__Node %empty
}

define internal %kelp_Box__Node @kelp_Box__Node__of(%tag_Node %0) {
entry:
  %b = alloca %kelp_Box__Node, align 8
  %gc = call ptr @kelp_gc_alloc(ptr @kelp_gc_type_Node)
  %oom = icmp eq ptr %gc, null
  br i1 %oom, label %oom1, label %alloc.ok

oom1:                                             ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.3)
  unreachable

alloc.ok:                                         ; preds = %entry
  store %tag_Node %0, ptr %gc, align 8
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %b, i32 0, i32 0
  store ptr %gc, ptr %value, align 8
  %of = load %kelp_Box__Node, ptr %b, align 8
  ret %kelp_Box__Node %of
}

define internal i1 @kelp_Box__Node__is_empty(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  ret i1 %empty
}

define internal ptr @kelp_Box__Node__get_const(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %empty2, label %get

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.4)
  unreachable

get:                                              ; preds = %entry
  ret ptr %value1
}

define internal void @kelp_Box__Node__set(ptr %0, %tag_Node %1) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %alloc, label %have

alloc:                                            ; preds = %entry
  %gc = call ptr @kelp_gc_alloc(ptr @kelp_gc_type_Node)
  %oom = icmp eq ptr %gc, null
  br i1 %oom, label %oom2, label %alloc.ok

have:                                             ; preds = %entry
  br label %store

store:                                            ; preds = %have, %alloc.ok
  %value3 = load ptr, ptr %value, align 8
  store %tag_Node %1, ptr %value3, align 8
  ret void

oom2:                                             ; preds = %alloc
  call void @kelp_panic(ptr @panic.msg.5)
  unreachable

alloc.ok:                                         ; preds = %alloc
  store ptr %gc, ptr %value, align 8
  br label %store
}

define internal %tag_Node @kelp_Box__Node__take(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %empty2, label %take

empty2:                                           ; preds = %entry
  call void @kelp_panic(ptr @panic.msg.6)
  unreachable

take:                                             ; preds = %entry
  %take3 = load %tag_Node, ptr %value1, align 8
  store ptr null, ptr %value, align 8
  ret %tag_Node %take3
}

define internal void @kelp_box_drop__Node(ptr %0) {
entry:
  %value = getelementptr inbounds nuw %kelp_Box__Node, ptr %0, i32 0, i32 0
  %value1 = load ptr, ptr %value, align 8
  %empty = icmp eq ptr %value1, null
  br i1 %empty, label %ret, label %live

ret:                                              ; preds = %live, %entry
  ret void

live:                                             ; preds = %entry
  store ptr null, ptr %value, align 8
  br label %ret
}

define i32 @main(i32 %0, ptr %1) !dbg !3 {
entry:
  %leaf = alloca %tag_Node, align 8, !dbg !6
  %tree = alloca %tag_Node, align 8, !dbg !6
  call void @kelp_gc_init(i64 8388608, i64 268435456, i64 1048576, i64 4194304, i64 20, i64 0), !dbg !6
  call void @kelp_gc_thread_attach(), !dbg !6
  store i32 %0, ptr @kelp_argc, align 4, !dbg !6
  store ptr %1, ptr @kelp_argv, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !7
  %call = call %kelp_Box__Node @kelp_Box__Node__empty(), !dbg !7
  %left = insertvalue %tag_Node undef, %kelp_Box__Node %call, 0, !dbg !7
  call void @kelp_gc_safepoint(), !dbg !7
  %call1 = call %kelp_Box__Node @kelp_Box__Node__empty(), !dbg !7
  %right = insertvalue %tag_Node %left, %kelp_Box__Node %call1, 1, !dbg !7
  store %tag_Node %right, ptr %leaf, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !8
  %leaf2 = load %tag_Node, ptr %leaf, align 8, !dbg !8
  %call3 = call %kelp_Box__Node @kelp_Box__Node__of(%tag_Node %leaf2), !dbg !8
  %left4 = insertvalue %tag_Node undef, %kelp_Box__Node %call3, 0, !dbg !8
  call void @kelp_gc_safepoint(), !dbg !8
  %call5 = call %kelp_Box__Node @kelp_Box__Node__empty(), !dbg !8
  %right6 = insertvalue %tag_Node %left4, %kelp_Box__Node %call5, 1, !dbg !8
  store %tag_Node %right6, ptr %tree, align 8, !dbg !6
  call void @kelp_gc_safepoint(), !dbg !9
  call void @kelp_gc_safepoint(), !dbg !9
  %left7 = getelementptr inbounds nuw %tag_Node, ptr %tree, i32 0, i32 0, !dbg !9
  %call8 = call i1 @kelp_Box__Node__is_empty(ptr %left7), !dbg !9
  %bool = select i1 %call8, ptr @str, ptr @str.7, !dbg !9
  %2 = call i32 (ptr, ...) @printf(ptr @str.8, ptr %bool), !dbg !9
  ret i32 0, !dbg !9
}

!llvm.dbg.cu = !{!0, !0}
!llvm.module.flags = !{!2}

!0 = distinct !DICompileUnit(language: DW_LANG_C, file: !1, producer: "kelpc 26.10.5", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug, splitDebugInlining: false)
!1 = !DIFile(filename: "node.kp", directory: "@FIXTURE_DIR@")
!2 = !{i32 2, !"Debug Info Version", i32 3}
!3 = distinct !DISubprogram(name: "main", linkageName: "main", scope: !1, file: !1, line: 7, type: !4, scopeLine: 7, flags: DIFlagPrototyped, spFlags: DISPFlagDefinition, unit: !0)
!4 = !DISubroutineType(flags: DIFlagPrototyped, types: !5)
!5 = !{}
!6 = !DILocation(line: 7, column: 1, scope: !3)
!7 = !DILocation(line: 8, column: 16, scope: !3)
!8 = !DILocation(line: 9, column: 16, scope: !3)
!9 = !DILocation(line: 10, column: 5, scope: !3)
