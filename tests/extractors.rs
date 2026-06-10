//! Golden tests for per-language reference extractors: what gets extracted,
//! how qualified calls split, scope attribution, builtin filtering,
//! and test-file detection.

use code_abyss::graph::languages::get_extractor;
use code_abyss::graph::{RawReference, RefKind};
use code_abyss::indexer::parser::MultiParser;

fn extract(lang: &str, source: &str) -> Vec<RawReference> {
    let parser = MultiParser::new();
    let tree = parser.parse(source, lang).unwrap();
    get_extractor(lang).unwrap().extract(&tree, source)
}

fn find<'a>(refs: &'a [RawReference], name: &str, kind: RefKind) -> Option<&'a RawReference> {
    refs.iter()
        .find(|r| r.target_name == name && r.kind == kind)
}

// --- Go ---

#[test]
fn go_direct_and_qualified_calls() {
    let refs = extract(
        "go",
        r#"package app

import "example.com/proj/util"

func Caller() int {
	x := Local()
	return util.Remote(x)
}
"#,
    );
    let local = find(&refs, "Local", RefKind::Call).expect("Local call");
    assert_eq!(local.target_qualifier, None);
    assert_eq!(local.source_symbol.as_deref(), Some("Caller"));

    let remote = find(&refs, "Remote", RefKind::Call).expect("Remote call");
    assert_eq!(remote.target_qualifier.as_deref(), Some("util"));

    let import = find(&refs, "example.com/proj/util", RefKind::Import).expect("import");
    assert_eq!(import.source_symbol, None);
}

#[test]
fn go_type_refs_and_builtin_filtering() {
    let refs = extract(
        "go",
        r#"package app

func Build(a Account) int {
	items := make([]int, 0)
	return len(items)
}
"#,
    );
    assert!(find(&refs, "Account", RefKind::TypeRef).is_some());
    assert!(
        find(&refs, "make", RefKind::Call).is_none(),
        "builtin make filtered"
    );
    assert!(
        find(&refs, "len", RefKind::Call).is_none(),
        "builtin len filtered"
    );
}

#[test]
fn go_test_file_detection() {
    let ex = get_extractor("go").unwrap();
    assert!(ex.is_test_file("pkg/handler_test.go"));
    assert!(!ex.is_test_file("pkg/handler.go"));
}

// --- Rust ---

#[test]
fn rust_path_call_splits_qualifier() {
    let refs = extract(
        "rust",
        "fn run() {\n    let x = crate::storage::open();\n    helper();\n}\n",
    );
    let open = find(&refs, "open", RefKind::Call).expect("path call");
    assert_eq!(open.target_qualifier.as_deref(), Some("crate::storage"));
    assert_eq!(open.source_symbol.as_deref(), Some("run"));

    let helper = find(&refs, "helper", RefKind::Call).expect("direct call");
    assert_eq!(helper.target_qualifier, None);
}

#[test]
fn rust_method_call_splits_receiver() {
    let refs = extract("rust", "fn run(repo: Repo) {\n    repo.commit();\n}\n");
    let commit = find(&refs, "commit", RefKind::Call).expect("method call split on '.'");
    assert_eq!(commit.target_qualifier.as_deref(), Some("repo"));
}

#[test]
fn rust_use_decl_and_builtin_filtering() {
    let refs = extract(
        "rust",
        "use crate::storage::Repository;\n\nfn run() -> Option<u32> {\n    Some(1)\n}\n",
    );
    assert!(find(&refs, "crate::storage::Repository", RefKind::Import).is_some());
    assert!(
        find(&refs, "Some", RefKind::Call).is_none(),
        "Some filtered"
    );
}

// --- Python ---

#[test]
fn python_attribute_call_and_imports() {
    let refs = extract(
        "python",
        "import util\nfrom pkg.mod import thing\n\ndef caller():\n    util.pfn()\n    local()\n",
    );
    let pfn = find(&refs, "pfn", RefKind::Call).expect("attribute call");
    assert_eq!(pfn.target_qualifier.as_deref(), Some("util"));
    assert_eq!(pfn.source_symbol.as_deref(), Some("caller"));

    assert!(find(&refs, "local", RefKind::Call).is_some());
    assert!(find(&refs, "util", RefKind::Import).is_some());
    assert!(find(&refs, "pkg.mod", RefKind::Import).is_some());
}

#[test]
fn python_builtin_filtering() {
    let refs = extract("python", "def f(xs):\n    print(len(xs))\n");
    assert!(find(&refs, "print", RefKind::Call).is_none());
    assert!(find(&refs, "len", RefKind::Call).is_none());
}

#[test]
fn python_test_file_detection() {
    let ex = get_extractor("python").unwrap();
    assert!(ex.is_test_file("pkg/test_handler.py"));
    assert!(ex.is_test_file("proj/tests/anything.py"));
    assert!(!ex.is_test_file("pkg/handler.py"));
}

// --- TypeScript / JavaScript ---

#[test]
fn ts_member_call_new_and_import() {
    let refs = extract(
        "typescript",
        "import { api } from './api';\n\nfunction caller(): Widget {\n  api.fetchUser();\n  build();\n  return new Widget();\n}\n",
    );
    let fetch = find(&refs, "fetchUser", RefKind::Call).expect("member call");
    assert_eq!(fetch.target_qualifier.as_deref(), Some("api"));
    assert_eq!(fetch.source_symbol.as_deref(), Some("caller"));

    assert!(find(&refs, "build", RefKind::Call).is_some());
    assert!(
        find(&refs, "Widget", RefKind::Call).is_some(),
        "new expression"
    );
    assert!(find(&refs, "./api", RefKind::Import).is_some());
    assert!(find(&refs, "Widget", RefKind::TypeRef).is_some());
}

#[test]
fn ts_builtin_filtering_and_test_detection() {
    let refs = extract(
        "typescript",
        "function f(x: string): number {\n  return parseInt(x);\n}\n",
    );
    assert!(find(&refs, "parseInt", RefKind::Call).is_none());
    assert!(find(&refs, "string", RefKind::TypeRef).is_none());

    let ex = get_extractor("typescript").unwrap();
    assert!(ex.is_test_file("src/api.test.ts"));
    assert!(ex.is_test_file("src/__tests__/api.ts"));
    assert!(!ex.is_test_file("src/api.ts"));
}

#[test]
fn javascript_uses_same_extractor() {
    let refs = extract("javascript", "function f() {\n  helper();\n}\n");
    assert!(find(&refs, "helper", RefKind::Call).is_some());
}

// --- Java ---

#[test]
fn java_method_calls_and_constructor() {
    let refs = extract(
        "java",
        r#"package app;

import com.example.util.Helper;

public class Service {
    public int run() {
        Helper h = new Helper();
        int x = h.compute(1);
        return local(x);
    }

    private int local(int x) { return x; }
}
"#,
    );
    let compute = find(&refs, "compute", RefKind::Call).expect("method call");
    assert_eq!(compute.target_qualifier.as_deref(), Some("h"));
    assert_eq!(compute.source_symbol.as_deref(), Some("run"));

    let ctor = find(&refs, "Helper", RefKind::Call).expect("constructor call");
    assert_eq!(ctor.target_qualifier, None);

    assert!(find(&refs, "local", RefKind::Call).is_some());
    assert!(find(&refs, "com.example.util.Helper", RefKind::Import).is_some());
    assert!(find(&refs, "Helper", RefKind::TypeRef).is_some());
}

#[test]
fn java_builtin_filtering_and_generics() {
    let refs = extract(
        "java",
        "import java.util.List;\n\npublic class A {\n    java.util.List<String> f() {\n        return new java.util.ArrayList<String>();\n    }\n}\n",
    );
    assert!(
        find(&refs, "ArrayList", RefKind::Call).is_none(),
        "builtin ctor filtered"
    );
    assert!(
        find(&refs, "String", RefKind::TypeRef).is_none(),
        "builtin type filtered"
    );
}

#[test]
fn java_test_file_detection() {
    let ex = get_extractor("java").unwrap();
    assert!(ex.is_test_file("src/test/java/app/ServiceTest.java"));
    assert!(ex.is_test_file("app/ServiceTest.java"));
    assert!(!ex.is_test_file("src/main/java/app/Service.java"));
}
