# AGENTS.md - Phoenix Kernel Development Guide

Guidelines for AI agents working on Phoenix, a bare-metal AArch64 kernel in Rust.

## Project Overview
Bare-metal AArch64 kernel with:
- `no_std` and `no_main` for bare-metal targets
- High-half virtual addressing (0xffffff8000000000)
- Assembly boot code with EL initialization and MMU setup
- Custom linker script for kernel memory layout
- Conditional compilation for host vs bare-metal
- Linux-style memblock boot-time allocator
- Thread-safe memory management with spin locks

## Build System

### Target Architecture
- Primary: `aarch64-unknown-none` (bare-metal AArch64)
- Host: Native for development/testing

### Build Commands
```bash
# Build for bare-metal AArch64
cargo build --target aarch64-unknown-none

# Build for host (development)
cargo build

# Release build for AArch64
cargo build --target aarch64-unknown-none --release

# Check code without building
cargo check --target aarch64-unknown-none

# Clean build artifacts
cargo clean
```

### Configuration Files
- `.cargo/config.toml`: Target-specific rustflags with custom linker script
- `build.rs`: Tracks architecture-specific file changes
- `src/arch/aarch64/kernel.ld`: Linker script for kernel memory layout
- `Cargo.toml`: Includes `spin = "0.9"` dependency for synchronization

## Testing

### Test Commands
```bash
# Run all tests on host
cargo test

# Run a specific test
cargo test test_target_guard

# Run tests with verbose output
cargo test -- --nocapture

# Test for AArch64 target
cargo test --target aarch64-unknown-none
```

### Test Structure
- Tests in `#[cfg(test)]` modules within source files
- Use `assert!`, `assert_eq!`, etc. for assertions
- Bare-metal tests may need QEMU/hardware
- Use `#[cfg(all(test, not(target_os = "none")))]` for host-only tests

## Code Style Guidelines

### Import Organization
```rust
// Core imports first
use core::arch::global_asm;
use core::panic::PanicInfo;

// Module declarations
mod arch;

// External crates
```

### Naming Conventions
- **Modules**: snake_case (`arch`, `aarch64`)
- **Functions**: snake_case (`kernel_main`, `panic`)
- **Constants**: SCREAMING_SNAKE_CASE (`KERNEL_VIRTUAL_BASE`)
- **Types**: PascalCase
- **Variables**: snake_case

### Error Handling
- Bare-metal: Use `panic!` for unrecoverable errors
- Implement `#[panic_handler]` in architecture modules
- Panic handlers should loop indefinitely

### Concurrency & Synchronization
- Use `spin::Mutex` for thread-safe global data structures
- Keep critical sections short to avoid deadlocks
- Document synchronization requirements for shared resources
- Example: `static MEMBLOCK: Mutex<Memblock> = Mutex::new(Memblock::new());`

### Memblock Implementation Guidelines
- Follow Linux memblock design: boot-time allocator before buddy system
- Use fixed-size arrays (e.g., `MAX_REGIONS = 128`) to avoid dynamic allocation
- Implement core operations: `add()`, `reserve()`, `remove()`, `alloc()`
- Merge adjacent regions automatically
- Check for overlaps when adding/reserving regions
- Support alignment requirements in `alloc()`
- Provide global instance via `spin::Mutex` for thread safety

### Unsafe Code
- Mark unsafe blocks with safety comments
- Use `unsafe` only for hardware access, raw pointers
- Document memory safety assumptions

```rust
unsafe {
    // Safety: 0xffff_ff80_0900_0000 is valid MMIO address for serial output
    core::ptr::write_volatile(0xffff_ff80_0900_0000 as *mut u8, *byte);
}
```

### Assembly Integration
- Assembly files: `.S` extension (uppercase for preprocessor)
- Include with `global_asm!(include_str!("boot.S"))`
- Add comprehensive comments explaining register operations

### Conditional Compilation
```rust
#[cfg(target_os = "none")]    // Bare-metal code
#[cfg(not(target_os = "none"))] // Host code
#[cfg(target_arch = "aarch64")] // AArch64 specific
```

## Architecture-Specific Code

### File Organization
```
src/
├── main.rs              # Entry point with conditional compilation
├── arch/
│   ├── mod.rs          # Architecture selector
│   └── aarch64/
│       ├── mod.rs      # AArch64 implementation
│       ├── boot.S      # Assembly boot code
│       └── kernel.ld   # Linker script
├── mm/
│   ├── mod.rs          # Memory management module
│   └── memblock.rs     # Boot-time allocator implementation
```

### Architecture Guidelines
- Each architecture in `src/arch/<arch-name>/`
- Conditionally compile with `#[cfg(target_arch = "...")]`
- Assembly needs detailed ARM system register comments
- Linker scripts define memory layout

## Development

### Cross-Compilation
```bash
rustup target add aarch64-unknown-none
```

### Debugging
- `cargo objdump --target aarch64-unknown-none -- -d` for disassembly
- Serial: 0xffff_ff80_0900_0000

## Code Quality

```bash
cargo clippy --target aarch64-unknown-none
cargo fmt
```

### Review Checklist
- [ ] Unsafe blocks with safety comments
- [ ] Correct conditional compilation
- [ ] Architecture-appropriate placement
- [ ] Well-commented assembly
- [ ] No std in bare-metal code
- [ ] Linker script matches memory map

## Commit Guidelines
- Use conventional commits: `feat:`, `fix:`, `chore:`, `docs:`, `test:`
- Reference architecture when applicable
- Keep commits focused on single logical changes

---
*Project: Phoenix AArch64 Kernel*