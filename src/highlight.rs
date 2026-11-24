//! Syntax highlighting with tree-sitter, colored like VS Code's Dark Modern theme.
//!
//! The grammar crates' own highlight queries are coarse (Go's has a single
//! `@keyword`), so each language gets extra patterns after them for the
//! distinctions VS Code makes: control-flow keywords (purple) vs declaration
//! keywords (blue), built-in types, calls, parameters, packages. Later patterns
//! win in tree-sitter-highlight, so catch-alls come first and specifics last. Brackets are colored by nesting depth
//! like VS Code's bracket-pair colorization.
use gpui::{HighlightStyle, Hsla, rgb};
use std::ops::Range;
use std::sync::OnceLock;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Capture names we color, most specific first (tree-sitter-highlight matches
/// each capture to the longest configured prefix).
const NAMES: &[&str] = &[
    "keyword.control",
    "keyword.operator",
    "keyword",
    "type.builtin",
    "type",
    "function.builtin",
    "function.method",
    "function",
    "variable.parameter",
    "variable.builtin",
    "variable",
    "property",
    "constant.builtin",
    "constant",
    "number",
    "string.special",
    "string",
    "escape",
    "comment",
    "operator",
    "label",
    "module",
    "constructor",
    "attribute",
    "preproc",
    "punctuation.special",
    "punctuation",
    "delimiter",
    "embedded",
    // Markdown (tree-sitter-md's queries).
    "text.title",
    "text.literal",
    "text.uri",
    "text.reference",
    "text.emphasis",
    "text.strong",
];

/// VS Code Dark Modern (Dark+) token colors for each name in [`NAMES`].
fn color_for(name: &str) -> Option<u32> {
    Some(match name {
        "keyword.control" => 0xC586C0,
        "keyword.operator" | "keyword" | "constant.builtin" | "variable.builtin" => 0x569CD6,
        "type.builtin" | "type" | "module" | "constructor" => 0x4EC9B0,
        "function.builtin" | "function.method" | "function" => 0xDCDCAA,
        "variable.parameter" | "variable" | "property" => 0x9CDCFE,
        "constant" => 0x4FC1FF,
        "number" => 0xB5CEA8,
        "string.special" | "string" => 0xCE9178,
        "escape" => 0xD7BA7D,
        "comment" => 0x6A9955,
        "operator" => 0xD4D4D4,
        "label" => 0xC8C8C8,
        "preproc" => 0xC586C0,
        "attribute" => 0xDCDCAA,
        // Markdown, as VS Code colors it: headings blue, code orange, links,
        // list and heading markers.
        "text.title" => 0x569CD6,
        "text.literal" => 0xCE9178,
        "text.uri" | "text.reference" => 0x3794FF,
        "punctuation.special" => 0x6796E6,
        _ => return None,
    })
}

const BRACKET_COLORS: [u32; 3] = [0xFFD700, 0xDA70D6, 0x179FFF];

const GO_EXTRA: &str = r##"
(field_identifier) @property
(identifier) @variable
(comment) @comment
(interpreted_string_literal) @string
(raw_string_literal) @string
(rune_literal) @string
(escape_sequence) @escape
["break" "case" "continue" "default" "defer" "else" "fallthrough" "for" "go" "goto" "if" "import" "package" "range" "return" "select" "switch"] @keyword.control
["chan" "const" "func" "interface" "map" "struct" "type" "var"] @keyword
((type_identifier) @type.builtin
  (#match? @type.builtin "^(any|bool|byte|comparable|complex64|complex128|error|float32|float64|int|int8|int16|int32|int64|rune|string|uint|uint8|uint16|uint32|uint64|uintptr)$"))
(type_identifier) @type
(package_clause (package_identifier) @module)
(call_expression function: (selector_expression field: (field_identifier) @function.method))
(call_expression function: (identifier) @function)
(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @function.method)
(parameter_declaration name: (identifier) @variable.parameter)
(variadic_parameter_declaration name: (identifier) @variable.parameter)
[(true) (false) (nil) (iota)] @constant.builtin
[(int_literal) (float_literal) (imaginary_literal)] @number
"##;

const C_EXTRA: &str = r##"
(field_identifier) @property
(identifier) @variable
(comment) @comment
(string_literal) @string
(char_literal) @string
(system_lib_string) @string
(escape_sequence) @escape
["if" "else" "for" "while" "do" "switch" "case" "default" "return" "break" "continue" "goto"] @keyword.control
["#include" "#define" "#if" "#ifdef" "#ifndef" "#else" "#elif" "#endif"] @keyword.control
(preproc_directive) @keyword.control
["struct" "enum" "union" "typedef" "static" "const" "extern" "inline" "volatile" "sizeof"] @keyword
(primitive_type) @keyword
(sized_type_specifier) @keyword
(type_identifier) @type
(call_expression function: (identifier) @function)
(call_expression function: (field_expression field: (field_identifier) @function))
(function_declarator declarator: (identifier) @function)
(parameter_declaration declarator: (identifier) @variable.parameter)
(parameter_declaration declarator: (pointer_declarator declarator: (identifier) @variable.parameter))
[(true) (false) (null)] @constant.builtin
(number_literal) @number
"##;

const CPP_EXTRA: &str = r##"
(field_identifier) @property
(identifier) @variable
(comment) @comment
(string_literal) @string
(raw_string_literal) @string
(char_literal) @string
(system_lib_string) @string
(escape_sequence) @escape
["if" "else" "for" "while" "do" "switch" "case" "default" "return" "break" "continue" "goto" "try" "catch" "throw" "co_return" "co_await" "co_yield"] @keyword.control
["#include" "#define" "#if" "#ifdef" "#ifndef" "#else" "#elif" "#endif"] @keyword.control
(preproc_directive) @keyword.control
["struct" "enum" "union" "typedef" "static" "const" "extern" "inline" "volatile" "sizeof" "class" "namespace" "template" "typename" "using" "public" "private" "protected" "virtual" "override" "final" "explicit" "friend" "operator" "new" "delete" "constexpr" "noexcept" "decltype" "mutable" "static_assert"] @keyword
(primitive_type) @keyword
(sized_type_specifier) @keyword
(auto) @keyword
(this) @variable.builtin
(namespace_identifier) @type
(type_identifier) @type
(call_expression function: (identifier) @function)
(call_expression function: (qualified_identifier name: (identifier) @function))
(call_expression function: (field_expression field: (field_identifier) @function))
(call_expression function: (template_function name: (identifier) @function))
(function_declarator declarator: (identifier) @function)
(function_declarator declarator: (field_identifier) @function)
(function_declarator declarator: (qualified_identifier name: (identifier) @function))
(parameter_declaration declarator: (identifier) @variable.parameter)
(parameter_declaration declarator: (reference_declarator (identifier) @variable.parameter))
(parameter_declaration declarator: (pointer_declarator declarator: (identifier) @variable.parameter))
[(true) (false) (null) (nullptr)] @constant.builtin
(number_literal) @number
"##;

const PYTHON_EXTRA: &str = r##"
(identifier) @variable
(comment) @comment
(string) @string
(escape_sequence) @escape
["if" "elif" "else" "for" "while" "return" "import" "from" "try" "except" "finally" "raise" "with" "break" "continue" "pass" "yield" "await" "assert" "del" "match" "case"] @keyword.control
["and" "or" "not" "in" "is"] @keyword.operator
["def" "class" "lambda" "global" "nonlocal" "async" "as"] @keyword
(decorator) @function
(call function: (identifier) @function)
(call function: (attribute attribute: (identifier) @function.method))
(function_definition name: (identifier) @function)
(class_definition name: (identifier) @type)
(parameters (identifier) @variable.parameter)
(typed_parameter (identifier) @variable.parameter)
(default_parameter name: (identifier) @variable.parameter)
((identifier) @variable.builtin (#eq? @variable.builtin "self"))
[(true) (false) (none)] @constant.builtin
[(integer) (float)] @number
(attribute attribute: (identifier) @property)
"##;

// VS Code Dark Modern for Rust: control flow purple, macros like functions,
// `self`/`mut` like keywords, parameters and fields light blue.
const RUST_EXTRA: &str = r##"
(parameter pattern: (identifier) @variable.parameter)
(field_identifier) @property
(shorthand_field_identifier) @property
(primitive_type) @type.builtin
(self) @variable.builtin
(mutable_specifier) @keyword
(lifetime (identifier) @type)
(macro_invocation macro: (identifier) @function "!" @function)
(macro_invocation macro: (scoped_identifier name: (identifier) @function) "!" @function)
["if" "else" "match" "for" "while" "loop" "return" "break" "continue" "in" "await"] @keyword.control
(try_expression "?" @keyword.control)
"##;

const BASH_EXTRA: &str = r##"
(comment) @comment
(string) @string
(raw_string) @string
["if" "then" "elif" "else" "fi" "for" "in" "do" "done" "while" "until" "case" "esac" "return" "exit"] @keyword.control
["function" "local" "export" "declare" "readonly" "unset"] @keyword
(command_name) @function
(function_definition name: (word) @function)
(variable_name) @variable
(simple_expansion) @variable
(expansion) @variable
(number) @number
"##;

struct Lang {
    config: HighlightConfiguration,
}

fn build(language: tree_sitter::Language, name: &str, extra: &str, base: &[&str]) -> Option<Lang> {
    // Later patterns win for the same node, so ours go after the grammar's.
    let query = base.iter().copied().chain(std::iter::once(extra)).collect::<Vec<_>>().join("\n");
    let mut config = match HighlightConfiguration::new(language.clone(), name, &query, "", "") {
        Ok(c) => c,
        Err(e) => {
            eprintln!("highlight query for {name} failed ({e:?}); using the grammar's own");
            HighlightConfiguration::new(language, name, &base.join("\n"), "", "").ok()?
        }
    };
    config.configure(NAMES);
    Some(Lang { config })
}

fn lang(name: &str) -> Option<&'static Lang> {
    macro_rules! cached {
        ($cell:ident, $build:expr) => {{
            static $cell: OnceLock<Option<Lang>> = OnceLock::new();
            $cell.get_or_init(|| $build).as_ref()
        }};
    }
    match name {
        "go" => cached!(GO, build(tree_sitter_go::LANGUAGE.into(), "go", GO_EXTRA, &[tree_sitter_go::HIGHLIGHTS_QUERY])),
        "c" => cached!(C, build(tree_sitter_c::LANGUAGE.into(), "c", C_EXTRA, &[tree_sitter_c::HIGHLIGHT_QUERY])),
        "cpp" => cached!(
            CPP,
            build(tree_sitter_cpp::LANGUAGE.into(), "cpp", CPP_EXTRA, &[tree_sitter_cpp::HIGHLIGHT_QUERY, tree_sitter_c::HIGHLIGHT_QUERY])
        ),
        // Starlark is a Python dialect; the web version used the Python grammar too.
        "python" | "starlark" => cached!(
            PY,
            build(tree_sitter_python::LANGUAGE.into(), "python", PYTHON_EXTRA, &[tree_sitter_python::HIGHLIGHTS_QUERY])
        ),
        "bash" => cached!(BASH, build(tree_sitter_bash::LANGUAGE.into(), "bash", BASH_EXTRA, &[tree_sitter_bash::HIGHLIGHT_QUERY])),
        "rust" => cached!(RUST, build(tree_sitter_rust::LANGUAGE.into(), "rust", RUST_EXTRA, &[tree_sitter_rust::HIGHLIGHTS_QUERY])),
        // Markdown is two grammars, blocks and inline text (see highlight_markdown).
        "markdown_block" => cached!(MD, build(tree_sitter_md::LANGUAGE.into(), "markdown", "", &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK])),
        "markdown_inline" => cached!(
            MD_INLINE,
            build(tree_sitter_md::INLINE_LANGUAGE.into(), "markdown_inline", "", &[tree_sitter_md::HIGHLIGHT_QUERY_INLINE])
        ),
        _ => None,
    }
}


fn style(hex: u32) -> HighlightStyle {
    HighlightStyle { color: Some(Hsla::from(rgb(hex))), ..Default::default() }
}

/// Highlights `text` (a whole file, or one side of a hunk). Returns byte ranges
/// with their styles, as gpui-component's diff view expects.
/// A fenced code block's language ("```go") as one of our grammars.
fn fence_grammar(name: &str) -> Option<&'static str> {
    Some(match name.to_ascii_lowercase().as_str() {
        "go" | "golang" => "go",
        "rust" | "rs" => "rust",
        "python" | "py" | "starlark" | "bzl" => "python",
        "bash" | "sh" | "shell" | "zsh" | "console" => "bash",
        "c" | "h" => "c",
        "cpp" | "c++" | "cc" | "cxx" | "hpp" => "cpp",
        _ => return None,
    })
}

/// Markdown: the block grammar's colors (headings, markers, code blocks),
/// then each paragraph's text with the inline grammar (emphasis, code spans,
/// links) and each fenced code block with its own language. These replace
/// the block colors underneath them.
fn highlight_markdown(text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut out = highlight("markdown_block", text);
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&tree_sitter_md::LANGUAGE.into()).is_err() {
        return out;
    }
    let Some(tree) = parser.parse(text, None) else { return out };
    let mut layers: Vec<(Range<usize>, Vec<(Range<usize>, HighlightStyle)>)> = Vec::new();
    let mut cursor = tree.walk();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        match node.kind() {
            // Headings keep their heading style.
            // Paragraph text and table cells.
            "inline" | "pipe_table_cell" if node.parent().is_some_and(|p| !p.kind().ends_with("heading")) => {
                let r = node.byte_range();
                layers.push((r.clone(), highlight("markdown_inline", &text[r])));
            }
            "fenced_code_block" => {
                let mut language = None;
                let mut content = None;
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "info_string" => language = child.child(0).map(|l| &text[l.byte_range()]),
                        "code_fence_content" => content = Some(child.byte_range()),
                        _ => {}
                    }
                }
                if let Some(r) = content {
                    // Code shows in its own colors, or plain: not as one orange block.
                    let inner = language.and_then(fence_grammar).map(|g| highlight(g, &text[r.clone()])).unwrap_or_default();
                    layers.push((r, inner));
                }
            }
            _ => stack.extend(node.children(&mut cursor).collect::<Vec<_>>()),
        }
    }
    for (range, inner) in layers {
        out.retain(|(r, _)| r.end <= range.start || r.start >= range.end);
        out.extend(inner.into_iter().map(|(r, s)| (r.start + range.start..r.end + range.start, s)));
    }
    out.sort_by_key(|(r, _)| r.start);
    out
}

pub fn highlight(language: &str, text: &str) -> Vec<(Range<usize>, HighlightStyle)> {
    if language == "markdown" {
        return highlight_markdown(text);
    }
    let Some(lang) = lang(language) else { return Vec::new() };
    let mut highlighter = Highlighter::new();
    let Ok(events) = highlighter.highlight(&lang.config, text.as_bytes(), None, |_| None) else {
        return Vec::new();
    };

    let mut out: Vec<(Range<usize>, HighlightStyle)> = Vec::new();
    // Ranges inside strings and comments, where brackets aren't code.
    let mut literal: Vec<Range<usize>> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for event in events.flatten() {
        match event {
            HighlightEvent::HighlightStart(h) => stack.push(h.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(&ix) = stack.last() {
                    let name = NAMES[ix];
                    if name.starts_with("string") || name == "comment" || name == "escape" {
                        literal.push(start..end);
                    }
                    match name {
                        "text.emphasis" => out.push((start..end, HighlightStyle { font_style: Some(gpui::FontStyle::Italic), ..Default::default() })),
                        "text.strong" => out.push((start..end, HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..Default::default() })),
                        "text.title" => out.push((start..end, HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), ..style(0x569CD6) })),
                        _ => {
                            if let Some(color) = color_for(name) {
                                out.push((start..end, style(color)));
                            }
                        }
                    }
                }
            }
        }
    }

    // Brackets in prose aren't code.
    if language.starts_with("markdown") {
        return out;
    }

    // Bracket-pair colors by depth, skipping brackets in strings/comments.
    // `literal` is in text order, so one moving index answers "inside a literal?".
    let mut lit = 0usize;
    let mut in_literal = |i: usize| {
        while lit < literal.len() && literal[lit].end <= i {
            lit += 1;
        }
        lit < literal.len() && literal[lit].contains(&i)
    };
    let mut depth = 0usize;
    let mut brackets = Vec::new();
    for (i, b) in text.bytes().enumerate() {
        match b {
            b'(' | b'[' | b'{' if !in_literal(i) => {
                brackets.push((i..i + 1, style(BRACKET_COLORS[depth % 3])));
                depth += 1;
            }
            b')' | b']' | b'}' if !in_literal(i) => {
                depth = depth.saturating_sub(1);
                brackets.push((i..i + 1, style(BRACKET_COLORS[depth % 3])));
            }
            _ => {}
        }
    }
    if !brackets.is_empty() {
        // Brackets override the plain punctuation color underneath.
        out.retain(|(r, _)| !(r.len() == 1 && matches!(text.as_bytes()[r.start], b'(' | b')' | b'[' | b']' | b'{' | b'}')));
        out.extend(brackets);
        out.sort_by_key(|(r, _)| r.start);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_control_flow_macros_and_types() {
        let src = "fn main() -> Result<(), Error> {\n    let mut x: u32 = 1;\n    if x > 0 { println!(\"{x}\"); }\n    run(&self.field)?;\n    Ok(())\n}\n";
        let hl = highlight("rust", src);
        let color_at = |needle: &str| {
            let at = src.find(needle).unwrap();
            hl.iter().find(|(r, _)| r.start <= at && at < r.end).and_then(|(_, s)| s.color)
        };
        assert_eq!(color_at("if x"), Some(Hsla::from(rgb(0xC586C0))), "control flow");
        assert_eq!(color_at("fn main"), Some(Hsla::from(rgb(0x569CD6))), "keyword");
        assert_eq!(color_at("println"), Some(Hsla::from(rgb(0xDCDCAA))), "macro");
        assert_eq!(color_at("u32"), Some(Hsla::from(rgb(0x4EC9B0))), "builtin type");
        assert_eq!(color_at("mut"), Some(Hsla::from(rgb(0x569CD6))), "mut");
        assert_eq!(color_at("?;"), Some(Hsla::from(rgb(0xC586C0))), "? operator");
    }

    #[test]
    fn go_keywords_types_functions() {
        let src = "package x\n\nfunc (s *Store) Run(dir string) error {\n\tif dir == \"\" {\n\t\treturn fmt.Errorf(\"x\")\n\t}\n\treturn nil\n}\n";
        let lang = lang("go").expect("go config");
        let mut h = Highlighter::new();
        let mut names = Vec::new();
        let mut stack = Vec::new();
        for ev in h.highlight(&lang.config, src.as_bytes(), None, |_| None).unwrap().flatten() {
            match ev {
                HighlightEvent::HighlightStart(x) => stack.push(x.0),
                HighlightEvent::HighlightEnd => { stack.pop(); }
                HighlightEvent::Source { start, end } => {
                    if let Some(&i) = stack.last() { names.push(format!("{}={}", &src[start..end], NAMES[i])); }
                }
            }
        }
        println!("{}", names.join("  "));
        let out = highlight("go", src);
        println!("{}", out.iter().map(|(r, st)| format!("{}:{:?}", &src[r.clone()], st.color.map(|c| gpui::Rgba::from(c)))).collect::<Vec<_>>().join("  "));
    }

    #[test]
    fn markdown_headings_code_links_and_fences() {
        let src = "# Title\n\nSome *soft* and **bold** text with `code` and [a link](https://x.y).\n\n```go\nfunc main() { return }\n```\n\n| A | B |\n|---|---|\n| `cell` | x |\n";
        let out = highlight("markdown", src);
        let at = |needle: &str| {
            let i = src.find(needle).unwrap();
            out.iter().find(|(r, _)| r.contains(&i)).map(|(_, s)| *s)
        };
        let title = at("Title").expect("heading styled");
        assert_eq!((title.color, title.font_weight), (Some(Hsla::from(rgb(0x569CD6))), Some(gpui::FontWeight::BOLD)));
        assert_eq!(at("soft").and_then(|s| s.font_style), Some(gpui::FontStyle::Italic));
        assert_eq!(at("bold").and_then(|s| s.font_weight), Some(gpui::FontWeight::BOLD));
        assert_eq!(at("code").and_then(|s| s.color), Some(Hsla::from(rgb(0xCE9178))));
        assert_eq!(at("https").and_then(|s| s.color), Some(Hsla::from(rgb(0x3794FF))));
        assert_eq!(at("return").and_then(|s| s.color), Some(Hsla::from(rgb(0xC586C0))), "Go inside a ```go fence");
        assert!(at("text with").is_none(), "plain prose stays plain");
        assert_eq!(at("cell").and_then(|s| s.color), Some(Hsla::from(rgb(0xCE9178))), "code in a table cell");
        assert!(at(" main()").is_none(), "code without a color stays plain, not one orange block");
    }
}