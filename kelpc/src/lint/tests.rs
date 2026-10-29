use super::*;

/// The runs as `(byte offset of the first comment, number of lines)`, which
/// is all a caller of a run needs.
fn runs(src: &str) -> Vec<(usize, usize)> {
    comment_runs(src)
        .expect("the fixture lexes")
        .into_iter()
        .map(|r| (r.span.start, r.lines))
        .collect()
}

/// The offset of `needle` in a fixture, so a test says *which* comment a run
/// starts at instead of counting bytes by hand.
fn at(src: &str, needle: &str) -> usize {
    src.find(needle).expect("the needle is in the fixture")
}

#[test]
fn two_consecutive_line_comments_are_one_run() {
    let src = "// one\n// two\nfun main() {}\n";
    assert_eq!(runs(src), vec![(at(src, "// one"), 2)]);
}

#[test]
fn a_run_counts_every_line_it_holds() {
    let src = "// one\n// two\n// three\n// four\n";
    assert_eq!(runs(src), vec![(at(src, "// one"), 4)]);
}

#[test]
fn a_single_line_comment_is_not_a_run() {
    assert_eq!(runs("// only one\nfun main() {}\n"), vec![]);
    assert_eq!(runs("fun main() {}\n// trailing\n"), vec![]);
}

#[test]
fn a_blank_line_between_two_comments_ends_the_run() {
    let src = "// one\n\n// two\n// three\n";
    assert_eq!(runs(src), vec![(at(src, "// two"), 2)]);
}

#[test]
fn code_between_two_comments_ends_the_run() {
    let src = "// one\nfun main() {}\n// two\n// three\n";
    assert_eq!(runs(src), vec![(at(src, "// two"), 2)]);
}

#[test]
fn a_comment_after_code_is_not_the_start_of_a_run() {
    // The first comment is a trailing one: its line holds code, so the two
    // lines are not a run of comment lines.
    let src = "val x = 1 // why\n// and another\n";
    assert_eq!(runs(src), vec![]);
}

#[test]
fn a_block_comment_between_line_comments_ends_the_run() {
    let src = "// one\n/* two */\n// three\n// four\n";
    assert_eq!(runs(src), vec![(at(src, "// three"), 2)]);
}

#[test]
fn a_doc_comment_run_is_not_a_run() {
    assert_eq!(runs("/// one\n/// two\nfun main() {}\n"), vec![]);
    assert_eq!(runs("//! one\n//! two\nfun main() {}\n"), vec![]);
}

#[test]
fn a_doc_comment_ends_a_run_of_ordinary_comments() {
    let src = "// one\n/// two\n/// three\n";
    assert_eq!(runs(src), vec![]);
}

#[test]
fn a_line_comment_inside_a_string_is_not_a_comment() {
    let src = "val s = \"// not a comment\"\nval t = \"// also not\"\n";
    assert_eq!(runs(src), vec![]);
}

#[test]
fn indented_comments_are_still_a_run() {
    // Indentation varies inside a run, and that is exactly what a block
    // comment would make uniform; the run is found anyway. The lexer's
    // newline token carries the next line's indentation, which is why the
    // rule counts terminators instead of measuring that span.
    let src = "fun main() {\n    // one\n        // two\n}\n";
    assert_eq!(runs(src), vec![(at(src, "// one"), 2)]);
}

#[test]
fn two_runs_are_reported_separately() {
    let src = "// one\n// two\n\nfun main() {}\n\n// three\n// four\n";
    assert_eq!(
        runs(src),
        vec![(at(src, "// one"), 2), (at(src, "// three"), 2)]
    );
}

#[test]
fn the_message_names_the_count_and_the_suggestion() {
    let src = "// one\n// two\n// three\n";
    let found = comment_runs(src).expect("lexes");
    assert_eq!(found.len(), 1);
    assert_eq!(
        found[0].message(),
        "3 consecutive '//' lines suggest one '/* ... */' comment"
    );
}

#[test]
fn a_lex_error_is_returned_rather_than_swallowed() {
    assert!(comment_runs("// one\n/* unterminated\n").is_err());
}

#[test]
fn the_carriage_return_of_a_crlf_file_does_not_break_a_run() {
    let src = "// one\r\n// two\r\n";
    assert_eq!(runs(src), vec![(0, 2)]);
}

/// The findings of a file linted under a case, as `(offset, message)`.
fn case_findings(src: &str, case: NamingCase) -> Vec<(usize, String)> {
    findings(
        src,
        Some(&Naming {
            case,
            from_manifest: true,
        }),
    )
    .expect("the fixture lexes")
    .into_iter()
    .map(|f| (f.span.start, f.message))
    .collect()
}

/// Just the names the naming rule reports, in source order.
fn named(src: &str, case: NamingCase) -> Vec<String> {
    case_findings(src, case)
        .into_iter()
        .map(|(_, message)| {
            let rest = message.split('\'').nth(1).unwrap_or_default();
            rest.to_string()
        })
        .collect()
}

#[test]
fn an_else_if_link_is_not_reported() {
    // `else if` is the preferred spelling (decision 110), so no rule warns.
    let src = "fun f(n: i32) {\n    if n > 1 { } else if n > 0 { }\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn an_else_if_let_link_is_not_reported() {
    let src = "fun f(m: Answer) {\n    if let Yes(_n) = m { } else if let No = m { }\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn elif_is_not_reported() {
    let src = "fun f(n: i32) {\n    if n > 1 { } elif n > 0 { } else { }\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn an_else_and_an_if_on_separate_lines_are_not_a_link() {
    // The parser reads the two tokens as a link only when they are
    // adjacent, so a separated pair is not the spelling this warns about.
    let src = "fun f(n: i32) {\n    if n > 1 { } else\n        if n > 0 { }\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn the_two_spellings_are_not_confused_inside_a_string() {
    let src = "fun f() {\n    print(\"if a { } else if b { }\")\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_borrow_on_a_list_or_str_parameter_is_reported_at_the_amp() {
    let src = "fun f(xs: &List<i32>, s: &str, o: &Point, d: &dyn Shape, n: i32) {\n}\n";
    let found = findings(src, None).expect("lexes");
    let starts: Vec<usize> = found.iter().map(|f| f.span.start).collect();
    assert_eq!(starts, vec![at(src, "&List"), at(src, "&str")]);
    assert!(found[0].message.starts_with("redundant '&' on a parameter"));
}

#[test]
fn a_borrow_outside_a_fun_header_is_not_reported() {
    let src = "fun f() {\n    val s: &str = \"a\"\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_comparison_with_true_is_reported_at_the_operator() {
    let src = "fun f(done: bool) {\n    if done == true { }\n}\n";
    let found = findings(src, None).expect("lexes");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].span.start, at(src, "== true"));
    assert_eq!(
        found[0].message,
        "'== true' is redundant: write the condition alone"
    );
}

#[test]
fn a_length_compared_with_zero_is_reported_at_the_receiver() {
    let src = "fun f(xs: List<i32>) {\n    if xs.len() == 0 { }\n}\n";
    let found = findings(src, None).expect("lexes");
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].span.start, at(src, "xs.len()"));
    assert_eq!(
        found[0].message,
        "'.len() == 0' reads better as '.is_empty()'"
    );
}

#[test]
fn the_other_comparisons_are_not_reported() {
    let src = "fun f(xs: List<i32>, n: i32, done: bool) {\n    if xs.len() > 0 && n == 0 && done == false { }\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_comparison_inside_a_string_is_not_reported() {
    let src = "fun f() {\n    val s = \"x == true and xs.len() == 0\"\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_bool_annotation_that_restates_a_literal_is_reported_at_the_type() {
    let src = "fun f() {\n    val ok: bool = true\n}\n";
    let found = findings(src, None).expect("lexes");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].span.start, at(src, "bool = true"));
    assert_eq!(
        found[0].message,
        "the annotation ': bool' restates the value: write the binding without it"
    );
}

#[test]
fn a_var_with_a_restated_false_is_reported_too() {
    let src = "fun f() {\n    var done: bool = false\n}\n";
    assert_eq!(findings(src, None).expect("lexes").len(), 1);
}

#[test]
fn a_constructor_annotation_that_restates_the_callee_is_reported() {
    let src = "datac Point(val x: i32)\nfun f() {\n    val p: Point = Point(x = 1)\n}\n";
    let found = findings(src, None).expect("lexes");
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].span.start, at(src, "Point = Point"));
    assert_eq!(
        found[0].message,
        "the annotation ': Point' restates the value: write the binding without it"
    );
}

#[test]
fn a_generic_annotation_is_never_reported() {
    let src = "datac Pair<A, B>(val first: A, val second: B)\nfun f() {\n    val p: Pair<i32, i32> = Pair(first = 1, second = 2)\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_call_to_another_name_is_not_a_restated_constructor() {
    let src = "datac Point(val x: i32)\nfun make() => Point {\n    return Point(x = 1)\n}\nfun f() {\n    val p: Point = make()\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_binding_without_an_annotation_or_with_a_needed_one_is_not_reported() {
    let src = "fun f() {\n    val ok = true\n    val n: int = 0\n    val s: str = \"x\"\n}\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

#[test]
fn a_module_level_pd_value_is_not_reported() {
    let src = "datac State(val a: i32)\npd val state: State = State(a = 7)\n";
    assert!(findings(src, None).expect("lexes").is_empty());
}

/// The names as owned strings, so an expected list reads as names.
fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| n.to_string()).collect()
}

#[test]
fn camel_case_reports_every_kind_of_declaration() {
    let src = "datac Point(val x_axis: i32)\n\nfun read_text(first_arg: i32) => i32 {\n    val line_count = 1\n    val f = fun(inner_arg: i32) = inner_arg\n    for each_item in 0 {\n        print(each_item)\n    }\n    return f(first_arg) + line_count + x_axis\n}\n";
    assert_eq!(
        named(src, NamingCase::Camel),
        strings(&[
            "x_axis",
            "read_text",
            "first_arg",
            "line_count",
            "inner_arg",
            "each_item",
        ])
    );
}

#[test]
fn a_field_named_by_two_shapes_is_reported_once() {
    // `datac P(val x_axis: i32)` is both a `val` binding and a field, and
    // they are the same identifier: one finding, not two.
    let src = "datac Point(val x_axis: i32, val y_axis: i32)\n";
    let found = case_findings(src, NamingCase::Camel);
    assert_eq!(found.len(), 2, "one finding per field, got {found:?}");
}

#[test]
fn a_destructuring_binding_declares_every_position_it_names() {
    // `val (lo_index, _) = ...` declares `lo_index` and ignores the other
    // position; a nested pattern declares its own names too, so the rule
    // reaches every name a tuple taken apart binds.
    let src = "fun f(p: (i32, i32)) => i32 {\n    val (lo_index, _) = p\n    val ((first_val, second_val), last_val) = ((1, 2), 3)\n    return lo_index + first_val + second_val + last_val\n}\n";
    assert_eq!(
        named(src, NamingCase::Camel),
        strings(&["lo_index", "first_val", "second_val", "last_val"])
    );
}

#[test]
fn a_tuple_type_is_not_a_declaration() {
    // `(i32, i32)` puts identifiers where a parameter list would, and a
    // type name is `PascalCase` whatever the option says, so no position
    // of a tuple type is this rule's business.
    let src = "fun f(lo_axis: (i32, DeviceInfo)) => (i32, i32) = (1, 2)\n";
    assert_eq!(named(src, NamingCase::Camel), strings(&["lo_axis"]));
}

#[test]
fn a_pattern_binding_is_checked_in_both_of_its_homes() {
    let src = "enum Shape { Circle(val r: f64) Empty }\n\nfun f(s: Shape) => f64 {\n    if let Circle(one_r) = s {\n        return one_r\n    }\n    when s {\n        Circle(two_r) -> return two_r\n        Empty -> return 0.0\n    }\n}\n";
    assert_eq!(
        named(src, NamingCase::Camel),
        strings(&["one_r", "two_r"])
    );
}

#[test]
fn a_call_is_not_a_declaration() {
    // The stdlib's locked `snake_case` is the reason this matters: a
    // project that asks for `camelCase` calls `str.is_empty` and is not
    // warned about a name it did not write and cannot change.
    let src = "fun f(s: str) => bool {\n    val empty = str.is_empty(s)\n    return empty\n}\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
}

#[test]
fn a_function_type_return_clause_is_not_a_binding() {
    // `fun(i32) => i32` puts an identifier right before a `:`. It is the
    // return clause of a function *type*, not a parameter, and the `.` in
    // front of the `R` is what says so.
    let src = "fun apply(f: fun(i32) => i32) => i32 = f(1)\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
}

#[test]
fn a_const_name_is_not_this_rules_business() {
    // `const LIMIT` is `UPPER_SNAKE` by convention (`syntax.md` 19), which
    // is neither of the option's two cases.
    let src = "const LIMIT: i32 = 10\n\nfun f() => i32 = LIMIT\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
    assert!(case_findings(src, NamingCase::Snake).is_empty());
}

#[test]
fn a_receivers_declaration_names_the_name_after_the_dot() {
    // The receiver's type name is `PascalCase` whatever the option says,
    // and the name after the `.` is a function name like any other; the
    // parameter is a binding.
    let src = "fun Circle.surface_area(scale_factor: f64) => f64 = scale_factor\n";
    assert_eq!(
        named(src, NamingCase::Camel),
        strings(&["surface_area", "scale_factor"])
    );
    assert!(case_findings(src, NamingCase::Snake).is_empty());
}

#[test]
fn a_function_literal_names_the_parameters_it_declares() {
    // A `fun` literal declares no name of its own, and its parameters are
    // bindings like any other.
    let src = "fun f() => i32 {\n    val g = fun(an_arg: i32) = an_arg\n    return g(1)\n}\n";
    assert_eq!(named(src, NamingCase::Camel), strings(&["an_arg"]));
}

#[test]
fn a_leading_underscore_is_not_part_of_either_case() {
    let src = "fun f() => i32 {\n    val _unused = 1\n    return _unused\n}\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
    assert!(case_findings(src, NamingCase::Snake).is_empty());
    // The underscore is kept in the suggestion, and the words after it
    // are what changes.
    let src = "fun f() => i32 {\n    val _unused_name = 1\n    return _unused_name\n}\n";
    assert!(case_findings(src, NamingCase::Camel)[0]
        .1
        .contains("write '_unusedName'"));
}

#[test]
fn a_manual_block_name_in_snake_case_is_a_binding() {
    // `block name: T { ... }` declares its name (`name.box()` reads it), so a
    // snake_case block name is a binding the default case warns about.
    let src = "fun f() => i32 {\n    val n = block request_node: Node {\n        return 1\n    }\n    return 0\n}\n";
    assert_eq!(named(src, NamingCase::Camel), strings(&["request_node"]));
}

#[test]
fn a_block_word_that_is_not_an_introducer_is_not_a_name() {
    // `block` is a contextual word: an ordinary name called `block` is a
    // binding, and it is in camelCase, so nothing is reported.
    let src = "fun f() => i32 {\n    val block: i32 = 5\n    return block\n}\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
}

#[test]
fn a_tuple_for_pattern_declares_every_name_it_binds() {
    // `for (lo_index, hi_index) in pairs` takes a tuple apart, and each name
    // in the pattern is a binding, like the names of `val (a, b) = ...`.
    let src = "fun f(pairs: str) => i32 {\n    for (lo_index, hi_index) in pairs {\n        print(lo_index)\n    }\n    return 0\n}\n";
    assert_eq!(
        named(src, NamingCase::Camel),
        strings(&["lo_index", "hi_index"])
    );
}

#[test]
fn a_type_and_a_constant_are_never_reported_under_the_default() {
    // A type is PascalCase whatever the option says, and a `const` is
    // UPPER_SNAKE by convention, so a snake-looking type name is not this
    // rule's business, and neither is a constant.
    let src = "datac point_box(val x: i32)\n\nconst MAX_LIMIT: i32 = 10\n\nenum shape_kind { Circle(val r: f64) Empty }\n";
    assert!(case_findings(src, NamingCase::Camel).is_empty());
}

#[test]
fn snake_case_asks_for_nothing() {
    // A project that asks for `snake_case` (`naming_case = 1`) is not warned
    // about any name: the lint checks the default case only, and a camelCase
    // name in a snake project is the project's own choice (#368 phase 3).
    let src = "fun readText() => i32 {\n    val lineCount = 1\n    return lineCount\n}\n";
    assert!(case_findings(src, NamingCase::Snake).is_empty());
    let mixed = "fun Read_Text(Line_Count: i32) => i32 = Line_Count\n";
    assert!(case_findings(mixed, NamingCase::Snake).is_empty());
}

#[test]
fn a_name_in_the_projects_case_is_left_alone() {
    let camel = "fun readText(lineCount: i32) => i32 = lineCount\n";
    assert!(case_findings(camel, NamingCase::Camel).is_empty());
    let snake = "fun read_text(line_count: i32) => i32 = line_count\n";
    assert!(case_findings(snake, NamingCase::Snake).is_empty());
}

#[test]
fn a_warning_that_came_from_a_manifest_says_so() {
    let src = "fun read_text() => i32 = 1\n";
    let from_manifest = case_findings(src, NamingCase::Camel)[0].1.clone();
    assert!(from_manifest.contains("coral.toml 'naming_case = 0'"), "{from_manifest}");
    let default = findings(src, Some(&Naming::default_without_manifest()))
        .expect("lexes")[0]
        .message
        .clone();
    assert!(default.contains("no manifest sets it"), "{default}");
}

#[test]
fn the_suggestions_are_the_case_rewrites() {
    assert_eq!(to_camel("read_text"), "readText");
    assert_eq!(to_camel("read__text"), "readText");
    assert_eq!(to_camel("_read_text"), "_readText");
}

#[test]
fn a_file_with_no_naming_setting_gets_no_naming_findings() {
    // What `src/deps/` gets: the comment rule still applies, the case rule
    // does not.
    let src = "// one\n// two\n\nfun read_text() => i32 = 1\n";
    let found = findings(src, None).expect("lexes");
    assert_eq!(found.len(), 1);
    assert!(found[0].message.contains("consecutive '//' lines"));
}

#[test]
fn an_impl_lines_for_is_not_a_loop_binding() {
    // `impl Name for Type { ... }` (A-12 slice 2) writes the same keyword a
    // `for` loop does, and only the loop binds a name: the contextual `in`
    // is what tells them apart, so a PascalCase `for` type is not read as a
    // mis-cased loop variable.
    let src = "trait Shape {\n    fun Self.area() => i32\n}\n\ndatac Circle(val r: i32)\n\nimpl Shape for Circle {\n    fun Self.area() => i32 = r\n}\n";
    let found = findings(
        src,
        Some(&Naming {
            case: NamingCase::Camel,
            from_manifest: false,
        }),
    )
    .expect("lexes");
    assert!(found.is_empty(), "{found:?}");
}

#[test]
fn the_findings_of_one_file_come_out_in_source_order() {
    let src = "fun read_text() => i32 {\n    // one\n    // two\n    if read_text() > 1 {\n        return 1\n    } else if read_text() == true {\n        return 2\n    }\n    return 3\n}\n";
    let found = findings(
        src,
        Some(&Naming {
            case: NamingCase::Camel,
            from_manifest: false,
        }),
    )
    .expect("lexes");
    assert_eq!(found.len(), 3, "{found:?}");
    let mut starts: Vec<usize> = found.iter().map(|f| f.span.start).collect();
    let sorted = starts.clone();
    starts.sort_unstable();
    assert_eq!(starts, sorted);
    assert_eq!(found[0].span.start, at(src, "read_text"));
}

/// The unused-binding findings as `(byte offset of the name, name)`.
fn unused(src: &str) -> Vec<(usize, String)> {
    findings(src, None)
        .expect("the fixture lexes")
        .into_iter()
        .filter(|f| f.message.starts_with("unused binding"))
        .map(|f| {
            let name = src[f.span.start..f.span.end].to_string();
            (f.span.start, name)
        })
        .collect()
}

#[test]
fn an_unused_binding_in_a_when_arm_is_reported_at_the_name() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Ok(v) -> v\n        Err(e) -> 0\n    }\n}\n";
    assert_eq!(unused(src), vec![(at(src, "e) -> 0"), "e".to_string())]);
}

#[test]
fn a_binding_used_in_its_own_arm_is_not_reported() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Ok(v) -> v\n        Err(e) -> e\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_name_starting_with_an_underscore_is_never_reported() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Ok(_v) -> 1\n        Err(_) -> 0\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_binding_of_an_if_let_is_reported_when_the_block_does_not_read_it() {
    let src = "fun f(o: Option<i32>) -> i32 {\n    if let Some(x) = o {\n        1\n    } else {\n        2\n    }\n}\n";
    assert_eq!(unused(src), vec![(at(src, "x) = o"), "x".to_string())]);
}

#[test]
fn a_binding_of_an_if_let_that_the_block_reads_is_not_reported() {
    let src = "fun f(o: Option<i32>) -> i32 {\n    if let Some(x) = o {\n        x\n    } else {\n        0\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_use_in_another_arm_does_not_count_for_this_arm() {
    // The second `e` is the second arm's own binding and reads it; the first
    // arm's `e` is still unread.
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Err(e) -> 0\n        Ok(e) -> e\n    }\n}\n";
    assert_eq!(unused(src), vec![(at(src, "e) -> 0"), "e".to_string())]);
}

#[test]
fn a_use_inside_a_string_interpolation_counts() {
    let src = "fun f(r: Result<i32, i32>) -> String {\n    when r {\n        Err(e) -> \"bad $e\"\n        Ok(_v) -> \"ok\"\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_use_in_a_block_body_counts_across_its_lines() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Err(e) -> {\n            val n = 1\n            n + e\n        }\n        Ok(v) -> v\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_body_continued_on_the_next_line_counts_as_one_arm() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Err(e) -> 0 +\n            e\n        Ok(v) -> v\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn arms_on_one_line_are_split_by_their_commas() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r { Ok(v) -> v, Err(e) -> 0 }\n}\n";
    assert_eq!(unused(src), vec![(at(src, "e) -> 0"), "e".to_string())]);
}

#[test]
fn the_finding_says_to_write_an_underscore() {
    let src = "fun f(r: Result<i32, i32>) -> i32 {\n    when r {\n        Err(e) -> 0\n        Ok(v) -> v\n    }\n}\n";
    let found = findings(src, None).expect("the fixture lexes");
    let message = &found.iter().find(|f| f.message.starts_with("unused binding")).expect("a finding").message;
    assert_eq!(
        message,
        "unused binding 'e' in a pattern: write '_' to ignore the value"
    );
}

#[test]
fn a_generic_fun_header_is_not_a_pattern_arm() {
    // `=>` after a parameter list is a return type, even when the name has
    // type arguments in front of the list.
    let src = "fun srv_err<T>(code: i32, text: &str) => Result<T, E> {\n    Err(code)\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_generic_header_with_bounds_is_not_a_pattern_arm() {
    let src = "pub fun map<I: Iterator, U>(it: move I, f: move closure fun(&I::Item) => U) => Map<I, U> {\n    return Map(inner = it, f = f)\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_select_case_is_a_call_and_not_a_pattern() {
    // `send(tx, x) -> 1` is a channel operation: `tx` and `x` are read.
    let src = "fun f(tx: Chan<i32>, x: i32) -> i32 {\n    select {\n        send(tx, x) -> 1,\n        else -> 0\n    }\n}\n";
    assert_eq!(unused(src), vec![]);
}

#[test]
fn a_let_else_is_not_an_if_let_block() {
    // The else block does not see the bindings, so the unread `user` here is
    // not reported from its block (the rule does not check let-else).
    let src = "fun f(id: i32) -> Result<i32, String> {\n    let Some(user) = find(id) else { return Err(\"missing\") }\n    return Ok(0)\n}\n";
    assert_eq!(unused(src), vec![]);
}
