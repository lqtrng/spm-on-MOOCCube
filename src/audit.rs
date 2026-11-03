//! Writes what a run calculates to `mined/<run>/`, at the moment it is calculated, so that a run which
//! later crashes still leaves everything it got to. Each run replaces the previous one of its kind.

use crate::data::{Itemset, Topic};
use crate::prefixspan::{Node, Patterns};
use std::fmt::Display;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

pub struct Audit {
    dir: PathBuf,
}

impl Audit {
    pub fn build(run: &str) -> Self {
        let dir = Path::new("mined").join(run);
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("clear previous audit");
        }
        fs::create_dir_all(&dir).expect("create audit dir");
        Audit { dir }
    }

    /// Writes a whole tab-separated file.
    pub fn table(&self, name: &str, header: &str, rows: impl IntoIterator<Item = impl Display>) {
        let mut out = self.create(name, header);
        for row in rows {
            writeln!(out, "{row}").expect("write audit table");
        }
        out.flush().expect("flush audit table");
    }

    /// Appends one row to a tab-separated file, on disk as soon as this returns.
    pub fn append(&self, name: &str, header: &str, row: impl Display) {
        let path = self.dir.join(name);
        let is_new = !path.exists();
        let mut out = OpenOptions::new().create(true).append(true).open(path).expect("open audit file");
        let header = if is_new { format!("{header}\n") } else { String::new() };
        write!(out, "{header}{row}\n").expect("append audit row");
    }

    /// One calculated value, in calculation order.
    pub fn log(&self, step: &str, value: impl Display) {
        self.append("log.tsv", "step\tvalue", format!("{step}\t{value}"));
    }

    /// Every pattern, depth first, with the supports that give `P(last item | prefix)`.
    pub fn patterns(&self, patterns: &Patterns) {
        let mut out = self.create("patterns.tsv", "pattern\tlength\tsupport\tprefix_support");
        write_children(&mut out, patterns, &patterns.nodes[0], "", 1).expect("write patterns");
        out.flush().expect("flush patterns");
    }

    fn create(&self, name: &str, header: &str) -> BufWriter<File> {
        let mut out = BufWriter::new(File::create(self.dir.join(name)).expect("create audit file"));
        writeln!(out, "{header}").expect("write audit header");
        out
    }
}

fn write_children(out: &mut impl Write, patterns: &Patterns, node: &Node, prefix: &str, len: usize) -> io::Result<()> {
    for c in patterns.children(node) {
        let pattern = if c.is_seq {
            format!("{prefix}({})", c.item)
        } else {
            format!("{},{})", &prefix[..prefix.len() - 1], c.item)
        };
        writeln!(out, "{pattern}\t{len}\t{}\t{}", c.support, node.support)?;
        write_children(out, patterns, c, &pattern, len + 1)?;
    }
    Ok(())
}

/// `1,2`
pub fn itemset(topics: &[Topic]) -> String {
    topics.iter().map(Topic::to_string).collect::<Vec<_>>().join(",")
}

/// `(1,2)(3)`
pub fn sequence(itemsets: &[Itemset]) -> String {
    itemsets.iter().map(|i| format!("({})", itemset(i))).collect()
}
