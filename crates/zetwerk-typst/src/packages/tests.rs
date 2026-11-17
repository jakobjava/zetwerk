use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use super::stand_in::{Registry, archive, manifest as toml_of, package};
use super::*;

/// `length` bytes that do not compress: a xorshift's.
pub(crate) fn noise(length: usize) -> Vec<u8> {
    let mut state: u64 = 0x2545_f491_4f6c_dd1d;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state >> 24) as u8
        })
        .collect()
}

/// A directory of the test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let directory =
        std::env::temp_dir().join(format!("zetwerk-packages-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

fn spec(text: &str) -> PackageSpec {
    parse_spec(text).unwrap()
}

/// Everything under `directory`, as paths from it.
fn everything(directory: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut directories = vec![directory.to_owned()];
    while let Some(here) = directories.pop() {
        for entry in std::fs::read_dir(&here).into_iter().flatten().flatten() {
            let path = entry.path();
            found.push(
                path.strip_prefix(directory)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
            if entry.file_type().unwrap().is_dir() {
                directories.push(path);
            }
        }
    }
    found.sort();
    found
}

/// An archive written header by header, as nothing that makes archives
/// would write it: names that lead out, links.
fn raw_archive(entries: &[(&[u8], tar::EntryType, &[u8], &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut builder = tar::Builder::new(encoder);
    for (name, kind, data, link) in entries {
        let mut header = tar::Header::new_old();
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.as_old_mut().linkname[..link.len()].copy_from_slice(link);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(*kind);
        header.set_cksum();
        builder.append(&header, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

#[test]
fn imports_of_packages_are_found_wherever_they_are() {
    let text = "#import \"@preview/touying:0.8.0\": *\n\
                #import \"chapter.typ\": thing\n\
                = Title\n\
                #let f() = { import \"@preview/cetz:0.3.2\" as draw; draw.line() }\n\
                #import \"@local/mine:1.0.0\": a, b as c\n\
                #import \"@preview/bad\": x\n";
    let root = typst::syntax::parse(text);
    let found = imports(&root);
    let specs: Vec<String> = found.iter().map(|import| import.spec.to_string()).collect();
    assert_eq!(
        specs,
        [
            "@preview/touying:0.8.0",
            "@preview/cetz:0.3.2",
            "@local/mine:1.0.0"
        ]
    );
    assert_eq!(found[0].names, Imported::All);
    assert_eq!(found[1].names, Imported::Module("draw".to_owned()));
    assert_eq!(
        found[2].names,
        Imported::Items(vec!["a".to_owned(), "b".to_owned()])
    );
    // The string that names it, quotes and all: what a newer version is
    // written into.
    assert_eq!(
        &text[found[0].literal.clone()],
        "\"@preview/touying:0.8.0\""
    );
    assert_eq!(&text[found[2].literal.clone()], "\"@local/mine:1.0.0\"");
}

#[test]
fn what_a_document_uses_of_a_package_is_found() {
    let text = "#import \"@preview/cetz:0.3.2\"\n\
                #import \"@preview/touying:0.8.0\": *\n\
                #import \"@preview/physica:0.9.3\": dd, pdv\n\
                #cetz.canvas(cetz.draw.line())\n\
                #slide[$dd x$]\n#title-slide()\n";
    let root = typst::syntax::parse(text);
    let found = imports(&root);
    let none = BTreeSet::new();
    let names = |set: BTreeSet<String>| set.into_iter().collect::<Vec<_>>();
    assert_eq!(names(used(&root, &found[0], &none)), ["canvas", "draw"]);
    let exported: BTreeSet<String> = ["slide", "title-slide", "focus-slide"]
        .map(str::to_owned)
        .into();
    assert_eq!(
        names(used(&root, &found[1], &exported)),
        ["slide", "title-slide"]
    );
    assert_eq!(names(used(&root, &found[2], &none)), ["dd"]);

    // What a package exports: what its entry point binds, and what it
    // takes from its own files.
    let dir = scratch("exports");
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("lib.typ"),
        "#import \"src/slides.typ\": *\n#import \"src/more.typ\": focus as focus-slide\n\
         #let title-slide(..args) = none\n#let (a, b) = (1, 2)\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("src/slides.typ"),
        "#let slide(body) = body\n#let hidden = { let inner = 1; inner }\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/more.typ"), "#let focus = 1\n").unwrap();
    assert_eq!(
        names(exports(&dir, "lib.typ")),
        ["a", "b", "focus-slide", "hidden", "slide", "title-slide"]
    );
    // An entry point that leads out is not read.
    assert!(exports(&dir, "../lib.typ").is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_package_says_what_it_is() {
    let dir = scratch("about");
    std::fs::write(
        dir.join("typst.toml"),
        toml_of(
            "thesis",
            "1.2.0",
            "homepage = \"https://example.org\"\n[template]\npath = \"template\"\nentrypoint = \"main.typ\"\n",
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("README.md"),
        "# Thesis\n\n[![badge](x)](y)\n![picture](p.png)\n\nA template for a thesis.\n\n```typ\ncode\n```\nIt has chapters.\n<img src=\"x\">\nMore.\nAnd more.\n",
    )
    .unwrap();
    let about = about(&dir).unwrap();
    assert_eq!(about.description.as_deref(), Some("The thesis package."));
    assert_eq!(about.authors, ["A. Writer"]);
    assert_eq!(about.license.as_deref(), Some("MIT"));
    assert_eq!(about.entrypoint, "lib.typ");
    assert_eq!(about.homepage.as_deref(), Some("https://example.org"));
    assert_eq!(
        about.template,
        Some(("template".to_owned(), "main.typ".to_owned()))
    );
    assert_eq!(
        about.readme,
        [
            "Thesis",
            "A template for a thesis.",
            "It has chapters.",
            "More."
        ]
    );
    std::fs::write(dir.join("typst.toml"), "[package]\nname = 3\n").unwrap();
    assert!(about_err(&dir).contains("typst.toml"));
    let _ = std::fs::remove_dir_all(dir);
}

fn about_err(dir: &Path) -> String {
    about(dir).err().unwrap_or_default()
}

#[test]
fn versions_are_ordered_as_numbers_and_the_index_is_searched() {
    let version = |text: &str| text.parse::<PackageVersion>().unwrap();
    assert!(version("0.10.0") > version("0.9.9"));
    assert!(version("1.0.0") > version("0.99.99"));
    assert!(version("0.3.10") > version("0.3.2"));
    let bytes = br#"[
        {"name": "cetz", "version": "0.3.2", "description": "Drawing with Typst", "keywords": ["draw", "canvas"]},
        {"name": "cetz", "version": "0.10.0", "description": "Drawing with Typst, newer"},
        {"name": "cetz", "version": "0.9.1", "description": "Drawing"},
        {"name": "fletcher", "version": "0.5.0", "description": "Diagrams with arrows, built on cetz"},
        {"name": "odd", "version": "0.2.0-dev", "description": "a version Typst cannot read"},
        {"name": "touying", "version": "0.8.0", "description": "Slides"}
    ]"#;
    let index = Index {
        listed: parse_index(bytes).unwrap(),
        when: SystemTime::now(),
    };
    assert_eq!(
        index.listed.len(),
        5,
        "the one Typst cannot read is left out"
    );
    assert_eq!(index.latest("cetz"), Some(version("0.10.0")));
    assert_eq!(index.latest("odd"), None);
    let found: Vec<String> = index
        .search("cetz")
        .iter()
        .map(|listed| format!("{} {}", listed.name, listed.version))
        .collect();
    // Its newest version, and the one whose name says it first.
    assert_eq!(found, ["cetz 0.10.0", "fletcher 0.5.0"]);
    let found = index.search("DRAWING typst");
    assert_eq!(found.len(), 1);
    assert_eq!(index.search("canvas")[0].version, version("0.3.2"));
    assert!(parse_index(b"{\"not\": \"a list\"}").is_err());
}

#[test]
fn addresses_are_made_of_the_name_and_the_version_alone() {
    assert_eq!(
        archive_url(REGISTRY, &spec("touying:0.8.0")).as_deref(),
        Some("https://packages.typst.org/preview/touying-0.8.0.tar.gz")
    );
    assert_eq!(
        archive_url("http://127.0.0.1:9/", &spec("@preview/my-pkg_2:1.10.3")).as_deref(),
        Some("http://127.0.0.1:9/preview/my-pkg_2-1.10.3.tar.gz")
    );
    assert_eq!(archive_url(REGISTRY, &spec("@local/mine:1.0.0")), None);
    assert_eq!(
        index_url(REGISTRY),
        "https://packages.typst.org/preview/index.json"
    );
    assert_eq!(said(&spec("touying:0.8.0")), "touying 0.8.0");
    assert_eq!(said(&spec("@local/mine:1.0.0")), "@local/mine 1.0.0");
    // A name is an identifier: nothing that leads elsewhere gets in.
    assert!(parse_spec("../x:1.0.0").is_err());
    assert!(parse_spec("@preview/a/b:1.0.0").is_err());
    assert!(parse_spec("touying").is_err());
}

#[test]
fn a_good_archive_is_put_in_place_whole() {
    let cache = scratch("good");
    let (_, bytes) = package("touying", "0.8.0", "#let slide(body) = body\n");
    let stop = AtomicBool::new(false);
    let target = install(
        &cache,
        &spec("touying:0.8.0"),
        &bytes,
        &stop,
        &mut create_file,
    )
    .unwrap();
    assert_eq!(target, cache.join("preview/touying/0.8.0"));
    assert_eq!(
        everything(&cache),
        [
            "preview",
            "preview/touying",
            "preview/touying/0.8.0",
            "preview/touying/0.8.0/lib.typ",
            "preview/touying/0.8.0/typst.toml"
        ]
    );
    // Archives made of a directory name their files `./…`.
    let dotted = archive(&[
        ("./typst.toml", toml_of("dots", "0.1.0", "").as_bytes()),
        ("./src/../lib.typ", b"x"),
    ]);
    let refused = install(
        &cache,
        &spec("dots:0.1.0"),
        &dotted,
        &stop,
        &mut create_file,
    );
    assert!(
        matches!(refused, Err(FetchError::Archive(_))),
        "{refused:?}"
    );
    let dotted = archive(&[
        ("./typst.toml", toml_of("dots", "0.1.0", "").as_bytes()),
        ("./lib.typ", b"x"),
    ]);
    install(
        &cache,
        &spec("dots:0.1.0"),
        &dotted,
        &stop,
        &mut create_file,
    )
    .unwrap();
    assert!(cache.join("preview/dots/0.1.0/lib.typ").is_file());
    // What is not the package asked for is refused.
    let other = install(
        &cache,
        &spec("other:0.1.0"),
        &bytes,
        &stop,
        &mut create_file,
    );
    assert!(
        matches!(&other, Err(FetchError::Archive(why)) if why.contains("touying 0.8.0")),
        "{other:?}"
    );
    let _ = std::fs::remove_dir_all(cache);
}

#[test]
fn an_archive_that_leads_out_is_refused_and_leaves_nothing() {
    let root = scratch("escape");
    let cache = root.join("cache");
    let manifest = toml_of("evil", "0.1.0", "");
    let stop = AtomicBool::new(false);
    let file = tar::EntryType::Regular;
    for (name, kind, link) in [
        (&b"../outside.txt"[..], file, &b""[..]),
        (b"src/../../outside.txt", file, b""),
        (b"/tmp/zetwerk-outside.txt", file, b""),
        (b"link", tar::EntryType::Symlink, b"../../../.."),
        (b"hard", tar::EntryType::Link, b"/etc/passwd"),
        (b"device", tar::EntryType::Char, b""),
    ] {
        let bytes = raw_archive(&[
            (b"typst.toml", file, manifest.as_bytes(), b""),
            (b"lib.typ", file, b"x", b""),
            (name, kind, b"escaped", link),
            (b"link/outside.txt", file, b"through the link", b""),
        ]);
        let refused = install(&cache, &spec("evil:0.1.0"), &bytes, &stop, &mut create_file);
        assert!(
            matches!(refused, Err(FetchError::Archive(_))),
            "{}: {refused:?}",
            String::from_utf8_lossy(name)
        );
        // Nothing of it, inside the cache or out of it.
        assert_eq!(
            everything(&root),
            ["cache", "cache/preview", "cache/preview/evil"]
        );
        assert!(!Path::new("/tmp/zetwerk-outside.txt").exists());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_cut_off_archive_or_a_full_disk_leaves_nothing() {
    let cache = scratch("cut");
    let lib = "x".repeat(200_000);
    let (_, bytes) = package("touying", "0.8.0", &lib);
    let stop = AtomicBool::new(false);
    for length in [0, 10, bytes.len() / 2, bytes.len() - 8] {
        let cut = install(
            &cache,
            &spec("touying:0.8.0"),
            &bytes[..length],
            &stop,
            &mut create_file,
        );
        assert!(
            matches!(cut, Err(FetchError::Archive(_))),
            "{length}: {cut:?}"
        );
        assert_eq!(everything(&cache), ["preview", "preview/touying"]);
    }
    // A disk that is full after some kilobytes.
    struct Full(usize);
    impl Write for Full {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if self.0 < bytes.len() {
                return Err(io::Error::new(io::ErrorKind::StorageFull, "no space left"));
            }
            self.0 -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut full = |path: &Path| -> io::Result<Box<dyn Write>> {
        // The file is made, as on a disk that fills up while it is
        // written.
        std::fs::write(path, b"")?;
        Ok(Box::new(Full(4096)))
    };
    let refused = install(&cache, &spec("touying:0.8.0"), &bytes, &stop, &mut full);
    assert!(
        matches!(&refused, Err(FetchError::Disk(why)) if why.contains("no space")),
        "{refused:?}"
    );
    assert_eq!(everything(&cache), ["preview", "preview/touying"]);
    // Asked to stop: nothing either.
    let stopped = AtomicBool::new(true);
    let refused = install(
        &cache,
        &spec("touying:0.8.0"),
        &bytes,
        &stopped,
        &mut create_file,
    );
    assert_eq!(refused, Err(FetchError::Stopped));
    assert_eq!(everything(&cache), ["preview", "preview/touying"]);
    let _ = std::fs::remove_dir_all(cache);
}

#[test]
fn a_package_and_what_it_needs_are_fetched_from_the_stand_in() {
    let root = scratch("fetch");
    let dirs = PackageDirs::within(&root);
    let (path, touying) = package(
        "touying",
        "0.8.0",
        "#import \"@preview/helper:0.2.0\": help\n#import \"@local/mine:1.0.0\"\n#let slide(b) = help(b)\n",
    );
    let (helper_path, helper) = package("helper", "0.2.0", "#let help(b) = b\n");
    let registry = Registry::serve(vec![(path, touying), (helper_path, helper)], None).unwrap();
    let ticks = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&ticks);
    let progress = Progress::new(move || {
        counted.fetch_add(1, Ordering::Relaxed);
    });
    let told = Mutex::new(Vec::new());
    let fetched = fetch_with_needs(
        &registry.url,
        &spec("touying:0.8.0"),
        &dirs,
        &progress,
        &|spec| told.lock().unwrap().push(said(spec)),
    )
    .unwrap();
    assert_eq!(fetched, [spec("touying:0.8.0"), spec("helper:0.2.0")]);
    assert_eq!(*told.lock().unwrap(), ["touying 0.8.0", "helper 0.2.0"]);
    assert!(ticks.load(Ordering::Relaxed) > 0, "progress is told");
    assert!(find(&dirs, &spec("helper:0.2.0")).is_some_and(|(_, place)| place == Place::Fetched));
    // What was asked: the two archives by name and version, and nothing
    // else (the `@local` one is not the registry's).
    let asked = registry.asked();
    let lines: Vec<&str> = asked
        .iter()
        .map(|asked| asked.lines().next().unwrap())
        .collect();
    assert_eq!(
        lines,
        [
            "GET /preview/touying-0.8.0.tar.gz HTTP/1.1",
            "GET /preview/helper-0.2.0.tar.gz HTTP/1.1"
        ]
    );
    assert!(asked[0].contains("zetwerk/"), "{}", asked[0]);
    // There already: not asked again.
    fetch_with_needs(
        &registry.url,
        &spec("touying:0.8.0"),
        &dirs,
        &progress,
        &|_| {},
    )
    .unwrap();
    assert_eq!(registry.asked().len(), 2);
    // What the registry does not have.
    let missing = fetch_with_needs(
        &registry.url,
        &spec("nothing:1.0.0"),
        &dirs,
        &progress,
        &|_| {},
    );
    assert_eq!(missing, Err((spec("nothing:1.0.0"), FetchError::NotThere)));
    // The list of packages, kept with its date.
    let listed = br#"[{"name": "touying", "version": "0.9.1", "description": "Slides"}]"#.to_vec();
    drop(registry);
    let registry = Registry::serve(vec![("/preview/index.json".to_owned(), listed)], None).unwrap();
    let index = fetch_index(&registry.url, dirs.index.as_deref(), &progress).unwrap();
    assert_eq!(index.latest("touying"), Some("0.9.1".parse().unwrap()));
    let kept = read_index(dirs.index.as_deref().unwrap()).unwrap();
    assert_eq!(kept.listed, index.listed);
    assert!(kept.when.elapsed().unwrap_or_default() < Duration::from_secs(60));
    // Nobody there at all.
    let url = registry.url.clone();
    drop(registry);
    let gone = fetch(&url, &spec("touying:0.9.1"), &root.join("cache"), &progress);
    assert!(matches!(gone, Err(FetchError::Network(_))), "{gone:?}");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_fetch_is_abandoned_while_it_comes() {
    let root = scratch("abandon");
    let dirs = PackageDirs::within(&root);
    let (path, bytes) = package("slow", "1.0.0", &"y".repeat(50_000));
    // Compressed, the 50 kB are small: more than that, of what does
    // not compress.
    let noise = noise(300_000);
    let noisy = archive(&[
        ("typst.toml", toml_of("slow", "1.0.0", "").as_bytes()),
        ("lib.typ", b"x"),
        ("noise.bin", &noise),
    ]);
    assert!(noisy.len() > 100_000 && !bytes.is_empty());
    let registry = Registry::serve(vec![(path, noisy)], Some(Duration::from_millis(5))).unwrap();
    let progress = Arc::new(Progress::default());
    let fetching = Arc::clone(&progress);
    let (url, cache) = (registry.url.clone(), dirs.cache.clone().unwrap());
    let thread = std::thread::spawn(move || fetch(&url, &spec("slow:1.0.0"), &cache, &fetching));
    let began = Instant::now();
    while progress.got().0 < 4096 && began.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let (got, total) = progress.got();
    assert!(
        got >= 4096 && total.is_some_and(|total| total > got),
        "{got} {total:?}"
    );
    progress.stop();
    assert_eq!(thread.join().unwrap(), Err(FetchError::Stopped));
    assert!(find(&dirs, &spec("slow:1.0.0")).is_none());
    assert!(everything(&root).iter().all(|path| !path.contains("tmp")));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn packages_on_the_machine_are_listed_and_only_fetched_ones_removed() {
    let root = scratch("listed");
    let dirs = PackageDirs::within(&root);
    let make = |base: &Path, name: &str, version: &str| {
        let dir = base.join(name).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("typst.toml"), toml_of(name, version, "")).unwrap();
        std::fs::write(dir.join("lib.typ"), "").unwrap();
    };
    let (data, cache) = (dirs.data.clone().unwrap(), dirs.cache.clone().unwrap());
    make(&data.join("local"), "mine", "1.0.0");
    make(&cache.join("preview"), "cetz", "0.3.2");
    make(&cache.join("preview"), "cetz", "0.10.0");
    make(&cache.join("preview"), "touying", "0.8.0");
    // Half unpacked, or no package at all: not listed.
    std::fs::create_dir_all(cache.join("preview/cetz/.tmp-zetwerk-0.4.0-1-0")).unwrap();
    std::fs::create_dir_all(cache.join("preview/not a name/1.0.0")).unwrap();
    let listed: Vec<String> = on_machine(&dirs)
        .iter()
        .map(|kept| format!("{} {}", kept.spec, kept.place.said()))
        .collect();
    assert_eq!(
        listed,
        [
            "@local/mine:1.0.0 your own",
            "@preview/cetz:0.10.0 fetched",
            "@preview/cetz:0.3.2 fetched",
            "@preview/touying:0.8.0 fetched"
        ]
    );
    // The writer's own are theirs: never removed.
    assert!(remove(&dirs, &spec("@local/mine:1.0.0")).is_err());
    assert!(find(&dirs, &spec("@local/mine:1.0.0")).is_some());
    // A version is removed and its sibling stays; the last takes the
    // package's directory along.
    remove(&dirs, &spec("cetz:0.3.2")).unwrap();
    assert!(find(&dirs, &spec("cetz:0.10.0")).is_some());
    remove(&dirs, &spec("touying:0.8.0")).unwrap();
    assert!(!cache.join("preview/touying").exists());
    assert!(remove(&dirs, &spec("touying:0.8.0")).is_err());
    // A link in the cache that leads elsewhere is not followed.
    let elsewhere = root.join("elsewhere/1.0.0");
    std::fs::create_dir_all(&elsewhere).unwrap();
    std::fs::create_dir_all(cache.join("preview/linked")).unwrap();
    std::os::unix::fs::symlink(&elsewhere, cache.join("preview/linked/1.0.0")).unwrap();
    assert!(remove(&dirs, &spec("linked:1.0.0")).is_err());
    assert!(elsewhere.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_template_is_copied_without_writing_over_anything() {
    let root = scratch("template");
    let package = root.join("package");
    std::fs::create_dir_all(package.join("template/figures")).unwrap();
    std::fs::write(
        package.join("typst.toml"),
        toml_of(
            "thesis",
            "1.0.0",
            "[template]\npath = \"template\"\nentrypoint = \"main.typ\"\n",
        ),
    )
    .unwrap();
    std::fs::write(
        package.join("template/main.typ"),
        "#import \"@preview/thesis:1.0.0\": *\n",
    )
    .unwrap();
    std::fs::write(package.join("template/refs.bib"), "@book{a,}\n").unwrap();
    std::fs::write(package.join("template/figures/logo.png"), [0_u8, 1, 2]).unwrap();
    let into = root.join("document");
    std::fs::create_dir_all(&into).unwrap();
    std::fs::write(into.join("refs.bib"), "mine\n").unwrap();
    let copied = copy_template(&package, &into).unwrap();
    assert_eq!(copied.text, "#import \"@preview/thesis:1.0.0\": *\n");
    assert_eq!(copied.made, [into.join("figures/logo.png")]);
    assert_eq!(copied.kept, [into.join("refs.bib")]);
    assert_eq!(
        std::fs::read_to_string(into.join("refs.bib")).unwrap(),
        "mine\n"
    );
    assert!(
        !into.join("main.typ").exists(),
        "its text is the document's"
    );
    // A template that leads out of its package is refused.
    std::fs::write(
        package.join("typst.toml"),
        toml_of(
            "thesis",
            "1.0.0",
            "[template]\npath = \"..\"\nentrypoint = \"main.typ\"\n",
        ),
    )
    .unwrap();
    assert!(copy_template(&package, &into).is_err());
    std::fs::write(package.join("typst.toml"), toml_of("thesis", "1.0.0", "")).unwrap();
    assert!(copy_template(&package, &into).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_document_compiles_with_the_packages_of_its_own_directories() {
    let root = scratch("compile");
    let dirs = PackageDirs::within(&root);
    let mine = dirs.data.clone().unwrap().join("local/mine/1.0.0");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::write(mine.join("typst.toml"), toml_of("mine", "1.0.0", "")).unwrap();
    std::fs::write(mine.join("lib.typ"), "#let greet = [Hello]\n").unwrap();
    let env = crate::Environment::with_packages(crate::FontSet::Embedded, dirs.clone());
    let text = "#import \"@local/mine:1.0.0\": greet\n#greet\n".to_owned();
    let mut project =
        crate::Project::in_memory(Arc::clone(&env), root.clone(), "main.typ", text).unwrap();
    let compiled = crate::compile(&project.snapshot());
    assert!(compiled.document.is_some(), "{:?}", compiled.errors);
    // One that is not there is not found, and nothing is asked for it.
    project.set_main_text("#import \"@preview/touying:0.8.0\": *\nText\n");
    let compiled = crate::compile(&project.snapshot());
    assert!(compiled.document.is_none());
    let said = format!("{:?}", compiled.errors);
    assert!(
        said.contains("package not found (searched for @preview/touying:0.8.0)"),
        "{said}"
    );
    // Put in place, it is found once the project is told.
    let (_, bytes) = package("touying", "0.8.0", "#let slide(b) = b\n");
    let stop = AtomicBool::new(false);
    install(
        &root.join("cache"),
        &spec("touying:0.8.0"),
        &bytes,
        &stop,
        &mut create_file,
    )
    .unwrap();
    project.packages_changed();
    let compiled = crate::compile(&project.snapshot());
    assert!(compiled.document.is_some(), "{:?}", compiled.errors);
    let _ = std::fs::remove_dir_all(root);
}
