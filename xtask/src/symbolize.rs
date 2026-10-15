//! Offline address-to-symbol lookup against the exact retained ELF artifact.
use super::*;

pub(super) fn run(arguments: Vec<String>) -> bool {
    let Some((elf, addresses)) = arguments.split_first() else {
        eprintln!("usage: cargo xtask symbolize <ELF> <0xADDRESS>...");
        return false;
    };
    if addresses.is_empty() {
        return false;
    }
    let Some(nm) = llvm_tool("llvm-nm") else {
        return false;
    };
    let Ok(output) = Command::new(nm)
        .args(["-S", "--defined-only", "--demangle", elf])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        eprintln!("{}", String::from_utf8_lossy(&output.stderr));
        return false;
    }
    let symbols = String::from_utf8_lossy(&output.stdout);
    addresses.iter().all(|raw| {
        let Ok(address) = u64::from_str_radix(raw.trim_start_matches("0x"), 16) else {
            eprintln!("invalid address: {raw}");
            return false;
        };
        match lookup(&symbols, address) {
            Some((name, offset)) => {
                println!("{address:#018x}: {name}+{offset:#x}");
                true
            }
            None => {
                eprintln!("{address:#018x}: no containing text symbol in {elf}");
                false
            }
        }
    })
}

fn lookup(symbols: &str, address: u64) -> Option<(&str, u64)> {
    symbols
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(4, ' ');
            let start = u64::from_str_radix(parts.next()?, 16).ok()?;
            let size = u64::from_str_radix(parts.next()?, 16).ok()?;
            if !matches!(parts.next()?, "T" | "t" | "W" | "w") {
                return None;
            }
            let name = parts.next()?;
            (start..start.checked_add(size)?)
                .contains(&address)
                .then_some((start, name))
        })
        .max_by_key(|(start, _)| *start)
        .map(|(start, name)| (name, address - start))
}

#[cfg(test)]
mod tests {
    #[test]
    fn lookup_requires_containing_text_and_keeps_demangled_names() {
        let symbols = "00001000 00000010 T phoenix::a b\n00002000 00000010 D data\n";
        assert_eq!(super::lookup(symbols, 0x1004), Some(("phoenix::a b", 4)));
        for pc in [0xfff, 0x1010, 0x2000] {
            assert_eq!(super::lookup(symbols, pc), None);
        }
    }
}
