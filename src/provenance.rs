//! Derived provenance (D9 / GT4): the tool revision, its dirty state, and the
//! dependency stack — all captured by `build.rs`, never typed by hand. This is
//! what makes every capture evidence (HGT2): a reader can name the exact tool
//! and the exact composed crates that produced it.

/// The build-derived provenance of this binary.
#[derive(Debug, Clone)]
pub struct Provenance {
    /// `lucid-host`'s own git revision at build (or `unknown`).
    pub tool_rev: &'static str,
    /// `dirty` if the tree had uncommitted changes at build, else `clean`.
    pub dirty: &'static str,
    /// Each composed dependency and its resolved git revision, or `LOCAL-PATH`.
    pub deps: &'static str,
}

impl Provenance {
    /// The provenance baked into this build.
    pub const fn current() -> Self {
        Provenance {
            tool_rev: env!("LUCID_HOST_REV"),
            dirty: env!("LUCID_HOST_DIRTY"),
            deps: env!("LUCID_HOST_DEPS"),
        }
    }

    /// True if any composed dependency resolved to a local path rather than a
    /// pinned git revision — a capture built this way is not reproducible and
    /// must say so.
    pub fn has_local_path_dep(&self) -> bool {
        self.deps.contains("LOCAL-PATH")
    }

    /// The provenance lines for a render or a capture header.
    pub fn lines(&self) -> String {
        format!(
            "tool: lucid-host {} ({})\ndeps: {}",
            self.tool_rev, self.dirty, self.deps
        )
    }

    /// Each `name@rev` dependency pair, split out for a structured header.
    pub fn dep_pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.deps
            .split(',')
            .filter(|s| !s.is_empty())
            .filter_map(|s| s.split_once('@'))
    }
}

impl Default for Provenance {
    fn default() -> Self {
        Self::current()
    }
}
