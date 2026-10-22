use super::*;

#[test]
fn an_interface_hash_ignores_private_bodies_but_tracks_public_signatures() {
    let first = "pub fun api(x: i32) => i32 = x\n\nfun hidden() => i32 = 1\n";
    let private_body_changed =
        "pub fun api(x: i32) => i32 = x\n\nfun hidden() => i32 = 999\n";
    let public_signature_changed =
        "pub fun api(x: i64) => i64 = x\n\nfun hidden() => i32 = 1\n";
    assert_eq!(
        interface_api_hash("main", first),
        interface_api_hash("main", private_body_changed)
    );
    assert_ne!(
        interface_api_hash("main", first),
        interface_api_hash("main", public_signature_changed)
    );
}

#[test]
fn an_interface_hash_tracks_bodies_that_leak_into_importers() {
    let edits = [
        // a generic template body
        ("pub fun id<T>(x: T) => T = x\n", "pub fun id<T>(x: T) => T = x\n\nfun hidden() => i32 = 2\n"),
        // a folded constant
        ("const N: i32 = 1\npub fun f() => i32 = N\n", "const N: i32 = 2\npub fun f() => i32 = N\n"),
        // a passdown's initializer
        ("pd val depth: i32 = 1\npub fun f() => i32 = depth\n", "pd val depth: i32 = 2\npub fun f() => i32 = depth\n"),
        // a destructor body
        ("datac B { v: i32 }\nfun mut B.drop() { }\n", "datac B { v: i32 }\nfun mut B.drop() { val x = 1 }\n"),
    ];
    for (before, after) in edits {
        assert_ne!(
            interface_api_hash("main", before),
            interface_api_hash("main", after),
            "an edit that reaches importers did not change the hash: {before}"
        );
    }
}

fn manifest(text: &str) -> Manifest {
    parse_manifest(text, Path::new("coral.toml")).expect("manifest should parse")
}

fn manifest_err(text: &str) -> Diagnostic {
    parse_manifest(text, Path::new("coral.toml"))
        .err()
        .expect("manifest should be rejected")
}

fn extern_section(module: &str, header: &str) -> String {
    format!(
        "[extern.\"{module}\"]\n\
         domain = \"time\"\n\
         library = \"c\"\n\
         symbols = []\n\
         header = {header}\n"
    )
}

#[test]
fn a_header_key_is_refused_as_removed_whatever_its_value() {
    for value in ["\"zlib.h\"", "zlib.h", "\"\"", "\"../evil.h\""] {
        let text = format!("name = \"x\"\n{}", extern_section("a", value));
        let err = manifest_err(&text);
        assert!(
            err.message.contains("'[extern.\"a\"] header' was removed"),
            "{value}: {}",
            err.message
        );
    }
}

fn extern_symbols_section(symbols: &str) -> String {
    format!(
        "name = \"x\"\n[extern.\"a\"]\n\
         domain = \"time\"\n\
         library = \"c\"\n\
         symbols = {symbols}\n"
    )
}

fn extern_symbol_of(symbols: &str, kelp: &str) -> Option<String> {
    let manifest = manifest(&extern_symbols_section(symbols));
    manifest
        .extern_modules
        .get("a")
        .expect("extern section")
        .symbol_of(kelp)
        .map(str::to_string)
}

#[test]
fn a_plain_symbol_entry_binds_the_kelp_name_to_itself() {
    assert_eq!(extern_symbol_of("[\"getpid\"]", "getpid"), Some("getpid".into()));
    assert_eq!(extern_symbol_of("[\"getpid\"]", "other"), None);
}

#[test]
fn a_renamed_symbol_entry_binds_the_kelp_name_to_the_c_symbol() {
    assert_eq!(
        extern_symbol_of("[\"window_new=gtk_window_new\"]", "window_new"),
        Some("gtk_window_new".into())
    );
    assert_eq!(
        extern_symbol_of("[\"window_new=gtk_window_new\"]", "gtk_window_new"),
        None
    );
}

#[test]
fn a_symbol_entry_with_an_empty_side_is_rejected() {
    for bad in ["[\"=getpid\"]", "[\"getpid=\"]"] {
        let err = manifest_err(&extern_symbols_section(bad));
        assert!(err.message.contains("must name a Kelp function and a C symbol"), "{bad}");
    }
}

#[test]
fn a_symbol_entry_with_whitespace_is_rejected() {
    for bad in ["[\"get pid\"]", "[\"getpid=gtk window_new\"]"] {
        let err = manifest_err(&extern_symbols_section(bad));
        assert!(err.message.contains("must not contain whitespace"), "{bad}");
    }
}

#[test]
fn a_symbol_entry_whose_c_symbol_is_not_an_identifier_is_rejected() {
    for bad in ["[\"getpid=a-b\"]", "[\"getpid=7z\"]", "[\"getpid=g(h)\"]"] {
        let err = manifest_err(&extern_symbols_section(bad));
        assert!(err.message.contains("must be a C identifier"), "{bad}");
    }
}

#[test]
fn a_kelp_name_listed_twice_is_rejected() {
    let err = manifest_err(&extern_symbols_section("[\"getpid\", \"getpid=other_pid\"]"));
    assert!(err.message.contains("'getpid' more than once"), "{}", err.message);
}

fn extern_dirs_section(module: &str, key: &str, value: &str) -> String {
    format!(
        "[extern.\"{module}\"]\n\
         domain = \"time\"\n\
         library = \"c\"\n\
         symbols = []\n\
         {key} = {value}\n"
    )
}

#[test]
fn an_include_dirs_key_is_refused_as_removed() {
    let err = manifest_err(&extern_dirs_section("a", "include_dirs", "[\"include\"]"));
    assert!(
        err.message.contains("'[extern.\"a\"] include_dirs' was removed"),
        "{}",
        err.message
    );
}

#[test]
fn library_dirs_are_parsed_as_written() {
    let parsed = manifest(&extern_dirs_section("a", "library_dirs", "[\"lib\", \"/opt/lib\"]"));
    assert_eq!(
        parsed
            .extern_modules
            .get("a")
            .expect("extern section")
            .library_dirs(),
        ["lib".to_string(), "/opt/lib".to_string()].as_slice()
    );
}

#[test]
fn a_dir_entry_that_is_not_a_bracketed_list_is_rejected() {
    let err = manifest_err(&extern_dirs_section("a", "library_dirs", "include"));
    assert!(err.message.contains("library_dirs' must be a bracketed list"), "{}", err.message);
}

#[test]
fn a_dir_entry_that_is_not_double_quoted_is_rejected() {
    let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[include]"));
    assert!(err.message.contains("library_dirs' must be double-quoted"), "{}", err.message);
}

#[test]
fn an_empty_dir_entry_is_rejected() {
    let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[\"\"]"));
    assert!(err.message.contains("library_dirs' must not be empty"), "{}", err.message);
}

#[test]
fn a_dir_entry_with_a_forbidden_character_is_rejected() {
    for bad in ["\"a\"b\"", "\"a\nb\"", "\"a\0b\""] {
        let err = parse_dir_list(&format!("[{bad}]"), "a", "library_dirs", Span::new(0, 0))
            .expect_err("expected a forbidden character to be refused");
        assert!(err.message.contains("must not contain"), "{bad:?}");
    }
}

#[test]
fn a_dir_entry_starting_with_a_dash_is_rejected() {
    let err = manifest_err(&extern_dirs_section("a", "library_dirs", "[\"-I/usr/include\"]"));
    assert!(err.message.contains("library_dirs' must not start with '-'"), "{}", err.message);
}

#[test]
fn extern_dirs_are_resolved_deduplicated_and_sorted() {
    let text = format!(
        "{}{}",
        extern_dirs_section("a", "library_dirs", "[\"include\", \"/opt/inc\"]"),
        extern_dirs_section("b", "library_dirs", "[\"include\"]"),
    );
    let parsed = manifest(&text);
    assert_eq!(
        collect_extern_dirs(
            &parsed.extern_modules,
            Some(Path::new("/proj")),
            |e| e.library_dirs()
        ),
        ["/opt/inc".to_string(), "/proj/include".to_string()].as_slice()
    );
}

#[test]
fn a_relative_dir_is_resolved_against_the_manifest_directory() {
    let parsed = manifest(&extern_dirs_section("a", "library_dirs", "[\"lib\"]"));
    let dirs = collect_extern_dirs(
        &parsed.extern_modules,
        Some(Path::new("/proj")),
        |e| e.library_dirs(),
    );
    assert_eq!(dirs, ["/proj/lib".to_string()].as_slice());
}

#[test]
fn pkg_config_packages_are_deduplicated_and_sorted() {
    let text = format!(
        "{}{}{}",
        extern_dirs_section("b", "pkg_config", "\"zlib\""),
        extern_dirs_section("a", "pkg_config", "\"gtk4\""),
        extern_dirs_section("c", "pkg_config", "\"zlib\""),
    );
    let parsed = manifest(&text);
    assert_eq!(
        collect_extern_pkg_config(&parsed.extern_modules),
        ["gtk4".to_string(), "zlib".to_string()].as_slice()
    );
}

#[test]
fn a_section_without_a_pkg_config_key_names_none() {
    let manifest = manifest(
        "name = \"x\"\n[extern.\"a\"]\n\
         domain = \"time\"\n\
         library = \"c\"\n\
         symbols = []\n",
    );
    assert!(collect_extern_pkg_config(&manifest.extern_modules).is_empty());
}

#[test]
fn a_pkg_config_that_is_not_a_quoted_string_is_rejected() {
    let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", "zlib"));
    let err = manifest_err(&text);
    assert!(err.message.contains("pkg_config' must be a double-quoted string"), "{}", err.message);
}

#[test]
fn an_empty_pkg_config_is_rejected() {
    let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", "\"\""));
    let err = manifest_err(&text);
    assert!(err.message.contains("pkg_config' must not be empty"), "{}", err.message);
}

#[test]
fn a_pkg_config_name_with_a_forbidden_character_is_rejected() {
    for bad in ["\"z lib\"", "\"zlib;rm\"", "\"z/lib\"", "\"zli\"b\""] {
        let text = format!("name = \"x\"\n{}", extern_dirs_section("a", "pkg_config", bad));
        let err = manifest_err(&text);
        assert!(
            err.message.contains("pkg_config' must contain only letters, digits and '._+-'"),
            "expected '{bad}' to be refused, got: {}",
            err.message
        );
    }
}

#[test]
fn a_ui_pattern_in_the_manifest_caps_section_is_rejected() {
    let err = manifest_err("name = \"x\"\n[caps]\nui = [\"wayland-0\"]\n");
    assert!(
        err.message.contains("patterns are not enforceable in the 'ui' domain yet"),
        "{}",
        err.message
    );
    // The wildcard is the vocabulary, so the section parses with it.
    assert_eq!(
        manifest("name = \"x\"\n[caps]\nui = [\"*\"]\n")
            .caps
            .expect("the [caps] section")
            .get("ui"),
        Some(&vec!["*".to_string()])
    );
}

#[test]
fn a_pkg_config_key_set_twice_is_rejected() {
    let text = "name = \"x\"\n[extern.\"a\"]\n\
                domain = \"time\"\n\
                library = \"c\"\n\
                symbols = []\n\
                pkg_config = \"zlib\"\n\
                pkg_config = \"gtk4\"\n"
        .to_string();
    let err = manifest_err(&text);
    assert!(err.message.contains("pkg_config' is set more than once"), "{}", err.message);
}

#[test]
fn the_build_section_turns_incremental_on_and_off() {
    // Roadmap A-14 slice 1: absent is the whole-program default, and the one
    // key the section has reads either boolean.
    assert!(!manifest("name = \"x\"\n").incremental);
    assert!(manifest("name = \"x\"\n[build]\nincremental = true\n").incremental);
    assert!(!manifest("name = \"x\"\n[build]\nincremental = false\n").incremental);
}

#[test]
fn the_build_section_names_an_obfuscation_mode() {
    use super::Obfuscate;
    assert_eq!(manifest("name = \"x\"\n").obfuscate, Obfuscate::Off);
    assert_eq!(
        manifest("name = \"x\"\n[build]\nobfuscate = \"names\"\n").obfuscate,
        Obfuscate::Names
    );
    assert_eq!(
        manifest("name = \"x\"\n[build]\nobfuscate = \"off\"\n").obfuscate,
        Obfuscate::Off
    );
    let err = manifest_err("name = \"x\"\n[build]\nobfuscate = \"strip\"\n");
    assert!(
        err.message.contains("must be \"off\" or \"names\""),
        "{}",
        err.message
    );
    let err = manifest_err("name = \"x\"\n[build]\nobfuscate = names\n");
    assert!(err.message.contains("must be \"off\" or \"names\""), "{}", err.message);
}

#[test]
fn the_build_section_names_a_sanitizer() {
    use crate::driver::Sanitizer;
    assert_eq!(manifest("name = \"x\"\n").sanitize, None);
    assert_eq!(
        manifest("name = \"x\"\n[build]\nsanitize = \"address\"\n").sanitize,
        Some(Sanitizer::Address)
    );
    let err = manifest_err("name = \"x\"\n[build]\nsanitize = \"memory\"\n");
    assert!(err.message.contains("must be \"address\""), "{}", err.message);
}

#[test]
fn a_malformed_build_section_is_rejected() {
    let err = manifest_err("name = \"x\"\n[build]\nincremental = \"true\"\n");
    assert!(
        err.message.contains("must be true or false"),
        "{}",
        err.message
    );
    let err = manifest_err("name = \"x\"\n[build]\nunits = true\n");
    assert!(
        err.message.contains("its keys are 'incremental', 'sanitize', 'emit', 'emit_entry' and 'obfuscate'"),
        "{}",
        err.message
    );
}

#[test]
fn the_build_section_chooses_what_is_emitted() {
    use super::Emit;
    assert_eq!(manifest("name = \"x\"\n").emit, Emit::Binary);
    assert_eq!(
        manifest("name = \"x\"\n[build]\nemit = \"html\"\nemit_entry = \"emit.kp\"\n").emit,
        Emit::Html
    );
    assert_eq!(
        manifest("name = \"x\"\n[build]\nemit = \"binary\"\n").emit,
        Emit::Binary
    );
    let err = manifest_err("name = \"x\"\n[build]\nemit = \"exe\"\n");
    assert!(
        err.message.contains("must be \"binary\" or \"html\""),
        "{}",
        err.message
    );
    let err = manifest_err("name = \"x\"\n[build]\nemit = html\n");
    assert!(err.message.contains("(double-quoted)"), "{}", err.message);
}

#[test]
fn html_emit_names_the_entry_it_compiles() {
    let m = manifest("name = \"x\"\n[build]\nemit = \"html\"\nemit_entry = \"emit.kp\"\n");
    assert_eq!(m.emit_entry.as_deref(), Some("emit.kp"));
    // The key order does not matter, and a binary build needs no entry.
    let m = manifest("name = \"x\"\n[build]\nemit_entry = \"emit.kp\"\nemit = \"html\"\n");
    assert_eq!(m.emit_entry.as_deref(), Some("emit.kp"));
    assert_eq!(manifest("name = \"x\"\n[build]\nemit = \"binary\"\n").emit_entry, None);
}

#[test]
fn html_emit_without_an_emit_entry_is_an_error() {
    let err = manifest_err("name = \"x\"\n[build]\nemit = \"html\"\n");
    assert!(
        err.message.contains("names no emit_entry"),
        "{}",
        err.message
    );
    let err = manifest_err("name = \"x\"\n[build]\nemit_entry = \"\"\nemit = \"html\"\n");
    assert!(err.message.contains("'[build] emit_entry'"), "{}", err.message);
}

#[test]
fn the_unsafe_section_lists_modules_in_order() {
    let m = manifest("name = \"x\"\n[unsafe]\nmodules = [\"kernel/mmio\", \"main\"]\n");
    assert_eq!(m.unsafe_modules, ["kernel/mmio".to_string(), "main".to_string()]);
    // No section means no module may contain `unsafe`.
    assert!(manifest("name = \"x\"\n").unsafe_modules.is_empty());
}

#[test]
fn a_malformed_unsafe_section_is_rejected() {
    let err = manifest_err("name = \"x\"\n[unsafe]\nmodules = \"main\"\n");
    assert!(
        err.message.contains("bracketed list of double-quoted module ids"),
        "{}",
        err.message
    );
    let err = manifest_err("name = \"x\"\n[unsafe]\nmods = [\"main\"]\n");
    assert!(err.message.contains("the section has one key, 'modules'"), "{}", err.message);
    let err = manifest_err("name = \"x\"\n[unsafe]\nmodules = [\"main\", \"main\"]\n");
    assert!(err.message.contains("appears more than once"), "{}", err.message);
}

#[test]
fn no_target_section_is_the_hosted_default() {
    let m = manifest("name = \"x\"\n");
    assert!(!m.target_seen);
    assert_eq!(m.target.mode(), TargetMode::Hosted);
    assert!(!m.target.is_freestanding());
    assert!(m.target.triple().is_none());
    assert!(m.target.entry().is_none());
    assert!(m.target.linker_script().is_none());
    assert!(m.target.panic().is_none());
}

#[test]
fn a_freestanding_target_section_is_read_whole() {
    let text = "name = \"x\"\n[target]\n\
                mode = \"freestanding\"\n\
                triple = \"riscv64-unknown-none-elf\"\n\
                entry = \"kmain\"\n\
                linker_script = \"kernel.ld\"\n\
                panic = \"kp_panic\"\n";
    let m = manifest(text);
    assert!(m.target.is_freestanding());
    assert_eq!(m.target.triple(), Some("riscv64-unknown-none-elf"));
    assert_eq!(m.target.entry(), Some("kmain"));
    assert_eq!(m.target.linker_script(), Some("kernel.ld"));
    assert_eq!(m.target.panic(), Some("kp_panic"));
}

#[test]
fn a_freestanding_target_section_reads_the_allocator_keys() {
    let text = "name = \"x\"\n[target]\n\
                mode = \"freestanding\"\n\
                triple = \"riscv64-unknown-none-elf\"\n\
                entry = \"kmain\"\n\
                linker_script = \"kernel.ld\"\n\
                alloc = \"kmalloc\"\n\
                dealloc = \"kfree\"\n\
                realloc = \"krealloc\"\n";
    let m = manifest(text);
    assert_eq!(m.target.alloc(), Some("kmalloc"));
    assert_eq!(m.target.dealloc(), Some("kfree"));
    assert_eq!(m.target.realloc(), Some("krealloc"));
    // A section with no allocator keys configures no allocator.
    assert!(manifest("name = \"x\"\n").target.alloc().is_none());
}

#[test]
fn the_cpu_count_is_read_as_a_bare_integer() {
    assert_eq!(manifest("name = \"x\"\n").target.cpus(), 1);
    let m = manifest(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
         linker_script = \"k.ld\"\ncpus = 4\n",
    );
    assert_eq!(m.target.cpus(), 4);
}

#[test]
fn a_cpu_count_outside_one_to_sixty_four_is_rejected() {
    for bad in ["0", "65", "yes", "\"4\""] {
        let err = manifest_err(&format!(
            "name = \"x\"\n[target]\nmode = \"hosted\"\ncpus = {bad}\n"
        ));
        assert!(
            err.message.contains("'[target] cpus'"),
            "{bad}: {}",
            err.message
        );
    }
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\ncpus = 2\ncpus = 4\n");
    assert!(err.message.contains("appears more than once"), "{}", err.message);
}

#[test]
fn a_half_allocator_pair_is_rejected() {
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
         linker_script = \"k.ld\"\nalloc = \"kmalloc\"\n",
    );
    assert!(err.message.contains("'alloc' without 'dealloc'"), "{}", err.message);
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\nentry = \"kmain\"\n\
         linker_script = \"k.ld\"\nrealloc = \"krealloc\"\n",
    );
    assert!(err.message.contains("'realloc' without 'alloc'"), "{}", err.message);
}

#[test]
fn a_hosted_target_section_names_no_freestanding_keys() {
    let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\n");
    assert_eq!(m.target.mode(), TargetMode::Hosted);
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"hosted\"\ntriple = \"riscv64-unknown-none-elf\"\n",
    );
    assert!(
        err.message.contains("only meaningful with 'mode = \"freestanding\"'"),
        "{}",
        err.message
    );
}

#[test]
fn a_target_section_without_a_mode_is_hosted() {
    let m = manifest("name = \"x\"\n[target]\n");
    assert!(m.target.has_shared_pool());
    assert!(!m.target.is_freestanding());
}

#[test]
fn mode_managed_is_refused_with_the_replacement() {
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"managed\"\n");
    assert!(err.message.contains("was removed"), "{}", err.message);
    assert!(err.message.contains("delete the mode line"), "{}", err.message);
}

#[test]
fn heap_max_is_accepted_in_a_hosted_section_and_refused_when_freestanding() {
    let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 64\n");
    assert_eq!(m.target.heap_max_mib(), 64);
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\n\
         entry = \"kmain\"\nlinker_script = \"k.ld\"\nheap_max = 64\n",
    );
    assert!(err.message.contains("only meaningful with 'mode = \"hosted\"'"), "{}", err.message);
}

fn a_hosted_target_section_is_accepted_with_the_default_heap() {
    let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\n");
    assert!(m.target.has_shared_pool());
    assert!(!m.target.is_freestanding());
    assert_eq!(m.target.heap_max_mib(), 256);
    assert_eq!(m.target.heap_max_mib(), crate::managed::DEFAULT_HEAP_MAX_MIB);
}

#[test]
fn heap_max_is_read_in_mib_for_a_hosted_section() {
    let m = manifest("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 512\n");
    assert_eq!(m.target.heap_max_mib(), 512);
}

#[test]
fn a_bad_heap_max_is_a_clear_error() {
    for bad in ["\"512\"", "abc", "-1", "0", "65537", "1.5"] {
        let err = manifest_err(&format!(
            "name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = {bad}\n"
        ));
        assert!(
            err.message.contains("'[target] heap_max'"),
            "{bad}: {}",
            err.message
        );
    }
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 0\n");
    assert!(err.message.contains("between 1 and 65536 MiB"), "{}", err.message);
}

#[test]
fn a_duplicate_heap_max_is_refused() {
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"hosted\"\nheap_max = 64\nheap_max = 128\n",
    );
    assert!(err.message.contains("appears more than once"), "{}", err.message);
}

#[test]
fn a_hosted_section_refuses_the_freestanding_and_allocator_keys() {
    for line in ["triple = \"x\"", "entry = \"main\"", "alloc = \"a\""] {
        let err = manifest_err(&format!("name = \"x\"\n[target]\nmode = \"hosted\"\n{line}\n"));
        assert!(err.message.contains("is only meaningful with 'mode = \"freestanding\"'"), "{}", err.message);
    }
}

#[test]
fn an_unknown_key_message_names_heap_max() {
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nnope = \"1\"\n");
    assert!(err.message.contains("'heap_max'"), "{}", err.message);
}

#[test]
fn a_freestanding_section_missing_a_required_key_is_rejected() {
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\nentry = \"kmain\"\n",
    );
    assert!(err.message.contains("missing 'triple"), "{}", err.message);
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\nlinker_script = \"k.ld\"\n",
    );
    assert!(err.message.contains("missing 'entry"), "{}", err.message);
}

#[test]
fn an_unknown_target_mode_is_rejected() {
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"based\"\n");
    assert!(err.message.contains("unknown 'based'"), "{}", err.message);
}

#[test]
fn a_target_entry_that_is_not_a_bare_name_is_rejected() {
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\n\
         triple = \"riscv64-unknown-none-elf\"\nentry = \"kernel.kmain\"\n\
         linker_script = \"k.ld\"\n",
    );
    assert!(err.message.contains("must be a bare function name"), "{}", err.message);
}

#[test]
fn an_unknown_key_in_the_target_section_is_rejected() {
    let err = manifest_err("name = \"x\"\n[target]\nmode = \"hosted\"\nnope = \"1\"\n");
    assert!(err.message.contains("unknown key 'nope'"), "{}", err.message);
}

#[test]
fn coral_package_metadata_is_skipped_and_unknown_flat_keys_stay_refused() {
    // `author`, `description` and the other `[package]` keys belong to coral,
    // which reads and checks them. kelpc skips the whole section, so a manifest
    // coral accepts is never refused here, whatever the metadata's shape.
    let text = "name = \"demo\"\nversion = \"0.1.0\"\n[package]\nauthor = [\"Ada Lovelace <ada@example.org>\"]\ndescription = \"A demo\"\nlicense = \"MIT\"\nrepository = \"https://example.org/demo.git\"\nhomepage = \"https://example.org/demo\"\nkelp = \"26.10.5\"\n";
    let m = manifest(text);
    assert_eq!(m.name.as_deref(), Some("demo"));
    assert_eq!(m.version.as_deref(), Some("0.1.0"));
    // The same key written at the top level is still a flat key, and kelpc
    // refuses it as it refuses every unknown flat key.
    let err = manifest_err("name = \"demo\"\nauthor = \"Ada <ada@example.org>\"\n");
    assert!(err.message.contains("unknown key 'author'"), "{}", err.message);
}

#[test]
fn alias_entries_are_read_from_the_alias_section() {
    let text = "name = \"x\"\n[alias]\nutil = \"helpers/util\"\n";
    let m = manifest(text);
    assert_eq!(m.alias.len(), 1);
    assert_eq!(m.alias[0].name, "util");
    assert_eq!(m.alias[0].text, "helpers/util");
    assert_eq!(m.alias[0].module, "coral.toml");
    let at = m.alias[0].text_offset;
    assert_eq!(&text[at..at + "helpers/util".len()], "helpers/util");
    assert!(m.alias_source.is_some());
}

#[test]
fn alias_paths_must_be_relative_and_non_empty() {
    for (value, needle) in [
        ("\"\"", "has an empty path"),
        ("\"/abs/util\"", "must be a relative module path"),
        ("\"../up\"", "must not contain '..'"),
        ("\"a/../b\"", "must not contain '..'"),
        ("util", "must be a double-quoted string"),
    ] {
        let err = manifest_err(&format!("[alias]\nu = {value}\n"));
        assert!(err.message.contains(needle), "{value}: {}", err.message);
    }
}

#[test]
fn a_duplicate_alias_in_the_section_is_rejected() {
    let err = manifest_err("[alias]\nu = \"a\"\nu = \"b\"\n");
    assert!(
        err.message.contains("alias 'u' is already defined in coral.toml [alias]"),
        "{}",
        err.message
    );
}

#[test]
fn a_manifest_without_an_alias_section_has_no_aliases() {
    let m = manifest("name = \"x\"\n");
    assert!(m.alias.is_empty());
    assert!(m.alias_source.is_none());
}

// Tidepool manifest settings: each accepted value, each refusal by name,
// and the section names that are refused rather than skipped.

#[test]
fn tidepool_defaults_apply_when_the_section_is_absent() {
    let m = manifest("name = \"x\"\n");
    let t = m.target.tidepool();
    assert_eq!(t.headroom_pct_or_default(), 10);
    assert_eq!(t.start_low_mib_or_default(), 1);
    assert_eq!(t.start_high_mib_or_default(), 4);
    assert_eq!(t.trigger_rule_or_default(), TriggerRule::TwoSize);
}

#[test]
fn tidepool_headroom_pct_accepts_both_ends_of_its_range() {
    let low = manifest("name = \"x\"\n[tidepool]\nheadroom_pct = 0\n");
    assert_eq!(low.target.tidepool().headroom_pct_or_default(), 0);
    let high = manifest("name = \"x\"\n[tidepool]\nheadroom_pct = 1000\n");
    assert_eq!(high.target.tidepool().headroom_pct_or_default(), 1000);
}

#[test]
fn tidepool_headroom_pct_above_1000_is_refused_by_name() {
    let err = manifest_err("name = \"x\"\n[tidepool]\nheadroom_pct = 1001\n");
    assert!(err.message.contains("key 'headroom_pct' must be from 0 to 1000, found '1001'"), "{}", err.message);
}

#[test]
fn tidepool_trigger_rule_accepts_each_rule_name_with_its_code() {
    for (name, code) in [("two-size", 0), ("last-size", 1), ("gogc-style", 2)] {
        let m = manifest(&format!("name = \"x\"\n[tidepool]\ntrigger_rule = \"{name}\"\n"));
        assert_eq!(m.target.tidepool().trigger_rule_or_default().code(), code, "{name}");
    }
}

#[test]
fn tidepool_start_high_equal_to_heap_max_is_accepted() {
    let m = manifest("name = \"x\"\n[target]\nheap_max = 64\n[tidepool]\nstart_high_mib = 64\n");
    assert_eq!(m.target.tidepool().start_high_mib_or_default(), 64);
}

#[test]
fn tidepool_start_high_above_heap_max_is_refused_by_name() {
    let err = manifest_err("name = \"x\"\n[target]\nheap_max = 8\n[tidepool]\nstart_high_mib = 9\n");
    assert!(
        err.message.contains("'start_high_mib' must not exceed [target] heap_max (8 MiB), found '9'"),
        "{}",
        err.message
    );
}

#[test]
fn tidepool_start_high_below_start_low_is_refused_by_name() {
    let err = manifest_err("name = \"x\"\n[target]\nheap_max = 8\n[tidepool]\nstart_low_mib = 3\nstart_high_mib = 2\n");
    assert!(
        err.message.contains("'start_high_mib' must be at least 'start_low_mib', which is '3', found '2'"),
        "{}",
        err.message
    );
}

#[test]
fn tidepool_duplicate_key_is_refused_by_name() {
    let err = manifest_err("name = \"x\"\n[tidepool]\nheadroom_pct = 10\nheadroom_pct = 20\n");
    assert!(err.message.contains("'headroom_pct' appears more than once"), "{}", err.message);
}

#[test]
fn an_empty_tidepool_section_is_accepted_hosted_and_freestanding() {
    let hosted = manifest("name = \"x\"\n[tidepool]\n");
    assert!(hosted.target.tidepool().is_unset());
    let free = manifest(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\n\
         entry = \"kmain\"\nlinker_script = \"k.ld\"\n[tidepool]\n",
    );
    assert!(free.target.is_freestanding());
}

#[test]
fn a_tidepool_key_is_refused_when_the_target_is_freestanding() {
    let err = manifest_err(
        "name = \"x\"\n[target]\nmode = \"freestanding\"\ntriple = \"riscv64-unknown-none-elf\"\n\
         entry = \"kmain\"\nlinker_script = \"k.ld\"\n[tidepool]\nheadroom_pct = 10\n",
    );
    assert!(
        err.message.contains("only meaningful with 'mode = \"hosted\"', because a freestanding build has no shared pool"),
        "{}",
        err.message
    );
}

#[test]
fn tidepool_section_names_about_tidepool_are_refused_by_name() {
    for header in [
        "[target.tidepool]",
        "[tidepool.limits]",
        "[Tidepool]",
        "[TIDEPOOL]",
        "[tidpool]",
        "[tid]",
        "[gc]",
        "[[tidepool]]",
    ] {
        let text = format!("name = \"x\"\n{header}\nheadroom_pct = 10\n");
        let err = manifest_err(&text);
        let name = header.trim_start_matches('[').trim_end_matches(']');
        let shown = if header.starts_with("[[") { format!("[[{name}]]") } else { format!("[{name}]") };
        assert!(
            err.message.contains(&format!("unknown section '{shown}'")),
            "{header}: {}",
            err.message
        );
    }
}

#[test]
fn other_sections_and_extern_modules_named_tidepool_are_not_refused() {
    let m = manifest("name = \"x\"\n[tasks]\nworkers = 4\n");
    assert!(m.target.tidepool().is_unset());
    let m = manifest("name = \"x\"\n[extern.\"tidepool_helper\"]\ndomain = \"time\"\nlibrary = \"c\"\nsymbols = []\n");
    assert!(m.extern_modules.contains_key("tidepool_helper"));
}
