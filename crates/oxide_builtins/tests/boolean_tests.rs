use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    vm.run(&Arc::new(module))
}

/// 求值一个应得布尔结果的表达式并断言其类型。
fn eval_bool(source: &str) -> bool {
    let result = eval(source).expect("eval should succeed");
    assert!(result.is_bool(), "expected boolean result, got {result:?}");
    result.as_bool()
}

/// [[IsHTMLDDA]] 宿主对象按 undefined 处理（B.3.4），Boolean() 应返 false。
#[test]
fn boolean_html_dda_is_falsy() {
    assert!(!eval_bool("Boolean($262.IsHTMLDDA)"));
}

/// 普通对象在 ToBoolean 下保持 truthy。
#[test]
fn boolean_plain_object_is_truthy() {
    assert!(eval_bool("Boolean({})"));
}
