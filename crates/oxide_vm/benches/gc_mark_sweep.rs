//! GC mark-sweep 微基准：单 VM 长 run 高分配率 churn，度量执行期两档收集
//! （epoch 晋升 + session 原地 sweep）。
//!
//! VM 在闭包外一次性构造、跨迭代复用（每迭代新建 VM 测的是构造 + 小 run，
//! 非 GC）；每迭代前全量重置使分配包络按 run 边界重起算，GC 统计与包络
//! 均为 per-run 口径，逐迭代读取累加，收尾摘要行即「GC 真触发」门禁
//! （收集次数 > 0）。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use oxide_compiler::compiler::Compiler;
use oxide_kernel::kernel::{KernelConfig, KernelCore};
use oxide_vm::vm::Vm;
use std::sync::Arc;

fn bench_gc(c: &mut Criterion) {
    // 低阈值：执行期两档收集按分配包络超水位触发，阈值即触发间距。
    let mut config = KernelConfig::minimal();
    config.session_gc_threshold = 512 * 1024;
    let kernel = KernelCore::new(config);

    // churn 源形态：每轮循环闭包 + 对象 + 数组，循环末全部死亡，
    // 单 run 约 15 万对象（50k × 3）。
    const N: usize = 50_000;
    let js = format!(
        "var t = 0; for (var i = 0; i < {N}; i++) {{ var f = function() {{ return i; }}; var o = {{ a: i, b: [i, i + 1] }}; t += f() + o.a + o.b.length; }} t"
    );
    let alloc = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&alloc, &js).expect("parse");
    let module = Arc::new(Compiler::new().compile(&program).expect("compile"));

    // 单 VM：闭包外构造，跨迭代复用。
    let mut vm = Vm::with_kernel_core(Arc::clone(&kernel));

    // per-run 口径统计：逐迭代累加，run 结束后读取。
    let mut collections = 0u64;
    let mut max_collection_us = 0u64;
    let mut objects_live = 0u64;
    let mut objects_dead = 0u64;
    let mut alloc_peak = 0usize;

    c.bench_function("gc_mark_sweep", |b| {
        b.iter(|| {
            // 每次迭代前全量重置：分配包络（run_alloc_peak）与水位按 run 边界
            // 重起算，逐迭代即单 run 口径；重置成本相对 churn run 可忽略。
            vm.full_reset();
            let result = vm.run(&module).expect("churn run");
            black_box(result);
            // 统计在 run 边界清零，逐迭代读取即该迭代口径。
            let stats = vm.session_gc_stats();
            collections += stats.total_collections;
            max_collection_us = max_collection_us.max(stats.max_collection_duration_us);
            objects_live += stats.total_objects_live;
            objects_dead += stats.total_objects_dead;
            alloc_peak = alloc_peak.max(vm.run_alloc_peak());
        });
    });

    eprintln!(
        "[gc_mark_sweep] collections={} max_collection_us={} live={} dead={} alloc_peak={}",
        collections, max_collection_us, objects_live, objects_dead, alloc_peak
    );
}

criterion_group!(benches, bench_gc);
criterion_main!(benches);
