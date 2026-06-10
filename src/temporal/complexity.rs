use tree_sitter::{Node, Tree};

pub fn cyclomatic_complexity(tree: &Tree, source: &str, language: &str) -> u32 {
    let mut complexity = 1u32;
    walk(tree.root_node(), source, language, &mut complexity);
    complexity
}

pub fn max_function_lines(tree: &Tree, source: &str, language: &str) -> u32 {
    let mut max_lines = 0u32;
    find_functions(&tree.root_node(), source, language, &mut max_lines);
    max_lines
}

fn walk(node: Node, source: &str, language: &str, complexity: &mut u32) {
    let kind = node.kind();
    if is_branch_node(kind, language) {
        if kind == "binary_expression" {
            if let Some(op) = node.child_by_field_name("operator") {
                let op_text = &source[op.start_byte()..op.end_byte()];
                if op_text == "&&" || op_text == "||" {
                    *complexity += 1;
                }
            }
        } else {
            *complexity += 1;
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, source, language, complexity);
    }
}

fn is_branch_node(kind: &str, language: &str) -> bool {
    match language {
        "go" => matches!(kind,
            "if_statement" | "for_statement" | "switch_statement" | "case_clause"
            | "select_statement" | "comm_clause" | "binary_expression"
        ),
        "rust" => matches!(kind,
            "if_expression" | "match_expression" | "match_arm"
            | "for_expression" | "while_expression" | "loop_expression"
            | "binary_expression"
        ),
        "typescript" | "javascript" | "tsx" => matches!(kind,
            "if_statement" | "for_statement" | "for_in_statement"
            | "while_statement" | "do_statement" | "switch_case"
            | "catch_clause" | "ternary_expression" | "binary_expression"
        ),
        "python" => matches!(kind,
            "if_statement" | "for_statement" | "while_statement"
            | "except_clause" | "conditional_expression" | "boolean_operator"
        ),
        _ => matches!(kind,
            "if_statement" | "for_statement" | "while_statement"
            | "switch_case" | "catch_clause"
        ),
    }
}

fn find_functions(node: &Node, source: &str, language: &str, max: &mut u32) {
    let kind = node.kind();
    let is_func = matches!(kind,
        "function_declaration" | "function_definition" | "function_item"
        | "method_declaration" | "method_definition"
    );
    if is_func {
        let lines = (node.end_position().row - node.start_position().row + 1) as u32;
        if lines > *max { *max = lines; }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        find_functions(&child, source, language, max);
    }
}
