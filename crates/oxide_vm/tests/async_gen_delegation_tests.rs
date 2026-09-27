//! 异步生成器 `yield*` 委托协议运行时测试：GetAsyncIterator 取迭代器、
//! 内层 next/return/throw 的 promise 结算、交付值二次 Await 展开、
//! return/throw 转发与失败路径（拒绝替换、TypeError 抛入 body 可 catch）。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::promise::promise_settled_value;
use oxide_vm::vm::Vm;

/// 执行源码并格式化顶层结果；Promise 结果取其 drain 后的结算值。
fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format_value(&vm, result),
        Err(e) => format!("vm error: {e}"),
    }
}

fn format_value(vm: &Vm, val: oxide_vm::JsValue) -> String {
    if val.is_string() {
        format!("\"{}\"", vm.lookup_str(val).unwrap_or_default())
    } else if val.is_object() {
        let obj = unsafe { &*val.as_js_object_ptr() };
        if obj.is_promise_obj() {
            match promise_settled_value(obj) {
                Some((true, v)) => format_value(vm, v),
                Some((false, v)) => format!("<rejected {}>", format_value(vm, v)),
                None => "<pending>".to_string(),
            }
        } else if obj.is_array() {
            "[array]".to_string()
        } else {
            "[object]".to_string()
        }
    } else {
        format!("{val}")
    }
}

// T1：基本委托——首个 next() 交付数字 7（非 IteratorResult 对象、非 promise），
// 第二个 next() 委托完成。
#[test]
fn t1_basic_async_iterable() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 7, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               return() { return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); const r1 = await it.next(); \
             const r2 = await it.next(); \
             return [r1.value, typeof r1.value, r1.done, r2.value, r2.done].join(','); })()"
        ),
        "\"7,number,false,,true\""
    );
}

// T2：多元素异步可迭代（顺序 promise 链），逐元素交付后 done。
#[test]
fn t2_multi_element_async_iterable() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             const items = [1, 2]; let i = 0; return { \
               next() { if (i < items.length) { return Promise.resolve({ value: items[i++], done: false }); } \
                         return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); \
             const a = (await it.next()).value; const b = (await it.next()).value; \
             const c = (await it.next()).done; return a + ',' + b + ',' + c; })()"
        ),
        "\"1,2,true\""
    );
}

// T3：同步可迭代进异步生成器——AsyncFromSyncIterator 回退路径同语义。
#[test]
fn t3_sync_iterable_fallback() {
    assert_eq!(
        eval(
            "async function* g() { yield* [1, 2, 3]; } \
             (async function run() { const it = g(); \
             const a = (await it.next()).value; const b = (await it.next()).value; \
             const c = (await it.next()).value; const d = (await it.next()).done; \
             return a + ',' + b + ',' + c + ',' + d; })()"
        ),
        "\"1,2,3,true\""
    );
}

// T4：二次 await——内层 next 交付 {value: Promise.resolve(5), done: false}，
// next() 交付展开后的 5。
#[test]
fn t4_double_await_unwraps_value() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; \
                        return Promise.resolve({ value: Promise.resolve(5), done: false }); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); return (await it.next()).value; })()"
        ),
        "5"
    );
}

// T5：交付值为拒绝 promise——yield* 不对值 Await，值原样让出，next() 以
// {value: 拒绝promise, done: false} 结算（不拒绝）。
#[test]
fn t5_rejected_yield_value_yielded_as_is() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; \
                        return Promise.resolve({ value: Promise.reject('e5'), done: false }); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); const r1 = await it.next(); \
             return r1.done + ':' + (r1.value instanceof Promise); })()"
        ),
        "\"false:true\""
    );
}

// T6：内层 next 拒绝且 body catch——拒绝原因抛入委托 body，可被捕获。
#[test]
fn t6_inner_next_reject_thrown_into_body() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.reject('e6'); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             catch (e) { return 'caught:' + e; } yield 'never'; } \
             (async function run() { const it = g(); const r = await it.next(); \
             return r.value + ':' + r.done; })()"
        ),
        "\"caught:e6:true\""
    );
}

// T7：内层结果非对象——TypeError 抛入委托 body，可被捕获。
#[test]
fn t7_non_object_result_thrown_into_body() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve(42); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             catch (e) { return 'caught:' + e.name; } } \
             (async function run() { const it = g(); const r = await it.next(); \
             return r.value + ':' + r.done; })()"
        ),
        "\"caught:TypeError:true\""
    );
}

// T8：.return(v) 转发——内层有 return 方法时以请求值调用，done 时生成器以
// 委托值（内层 return 结果值）完成（非请求值）。
#[test]
fn t8_return_forwarded_completes_with_inner_value() {
    assert_eq!(
        eval(
            "var log = []; \
             async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               return(v) { log.push('ret:' + v); return Promise.resolve({ value: 'inner', done: true }); } }; } }; } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.return(9); return r.value + ':' + r.done + ':' + log.join(','); })()"
        ),
        "\"inner:true:ret:9\""
    );
}

// T9：.return(v) 内层无 return 方法——同步完成路径，直接以请求值完成。
#[test]
fn t9_return_without_inner_return() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.return(9); return r.value + ':' + r.done; })()"
        ),
        "\"9:true\""
    );
}

// T10：.throw(e) 转发——内层有 throw 方法时以 e 调用，未 done 时交付内层值。
#[test]
fn t10_throw_forwarded_to_inner() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               throw(e) { return Promise.resolve({ value: 'x:' + e, done: false }); } }; } }; } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.throw('e10'); return r.value + ':' + r.done; })()"
        ),
        "\"x:e10:false\""
    );
}

// T11：.throw(e) 内层无 throw 方法——close 成功后抛 TypeError 入 body（协议违规，
// 非原异常），可 catch。
#[test]
fn t11_throw_without_inner_throw_close_then_typeerror() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               return() { return Promise.resolve({ done: true }); } }; } }; } \
             catch (e) { return 'caught:' + e.name; } } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.throw('e11'); return r.value + ':' + r.done; })()"
        ),
        "\"caught:TypeError:true\""
    );
}

// T12：close 拒绝——.throw(e) 内层无 throw 方法且内层 return() 拒绝时，拒绝原因
// 抛入 body（可 catch），生成器以 catch 返回值完成，throw 请求以该值结算。
#[test]
fn t12_close_rejection_thrown_into_body() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               return() { return Promise.reject('r12'); } }; } }; } \
             catch (e) { return 'caught:' + e; } } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.throw('e12'); return r.value + ':' + r.done; })()"
        ),
        "\"caught:r12:true\""
    );
}

// T13：return 分支 done:false——内层 return 结果非 done 时，委托值原样让出
// （return 请求以 {value: 9, done: false} 结算）。
#[test]
fn t13_return_not_done_yields_inner_value() {
    assert_eq!(
        eval(
            "async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; return Promise.resolve({ value: 1, done: false }); } \
                        return Promise.resolve({ done: true }); }, \
               return(v) { return Promise.resolve({ value: 9, done: false }); } }; } }; } \
             (async function run() { const it = g(); await it.next(); \
             const r = await it.return(5); return r.value + ':' + r.done; })()"
        ),
        "\"9:false\""
    );
}

// T14：微任务时序——内层结算先于外层 next() 结算（push 序数组断言交错）。
#[test]
fn t14_microtask_ordering() {
    assert_eq!(
        eval(
            "var order = []; \
             async function* g() { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; \
                        return new Promise(r => { order.push('inner'); r({ value: 1, done: false }); }); } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             (async function run() { const it = g(); const p = it.next(); \
             order.push('req'); const r = await p; order.push('outer'); \
             return order.join(','); })()"
        ),
        "\"inner,req,outer\""
    );
}

// 边界：委托不可迭代值——TypeError 抛入委托 body，可被捕获。
#[test]
fn t15_yield_star_non_iterable_thrown_into_body() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* 42; } catch (e) { return 'caught:' + e.name; } } \
             (async function run() { const it = g(); const r = await it.next(); \
             return r.value + ':' + r.done; })()"
        ),
        "\"caught:TypeError:true\""
    );
}

// 边界：内层 next 同步抛错（非 promise 拒绝）——异常抛入委托 body，可被捕获。
#[test]
fn t16_inner_next_sync_throw_thrown_into_body() {
    assert_eq!(
        eval(
            "async function* g() { try { yield* { [Symbol.asyncIterator]() { \
             let first = true; return { \
               next() { if (first) { first = false; throw 'sync-e'; } \
                        return Promise.resolve({ done: true }); } }; } }; } \
             catch (e) { return 'caught:' + e; } } \
             (async function run() { const it = g(); const r = await it.next(); \
             return r.value + ':' + r.done; })()"
        ),
        "\"caught:sync-e:true\""
    );
}
