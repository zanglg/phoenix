# Phoenix: A Linux-like Kernel in Rust

[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-2024-edition-orange.svg)](https://www.rust-lang.org/)
[![Target](https://img.shields.io/badge/target-aarch64--unknown--none-green.svg)](https://doc.rust-lang.org/nightly/rustc/platform-support/aarch64-unknown-none.html)

> **AI-Generated Project**: This kernel is being developed with significant AI assistance through the [opencode](https://opencode.ai) platform. The development follows an iterative, research-oriented approach exploring Rust's capabilities for safe systems programming.

## 🎯 Project Vision

Phoenix aims to be a **research-oriented, Linux-like kernel** written entirely in Rust, exploring the boundaries of memory-safe systems programming while maintaining architectural similarity to the Linux kernel. Unlike traditional kernels, Phoenix leverages Rust's ownership model and type system to eliminate entire classes of memory safety bugs while providing similar capabilities and interfaces.

### Core Research Questions:
- Can Rust's safety guarantees be maintained in a production-quality kernel?
- How does a memory-safe kernel impact performance and complexity?
- What architectural adaptations are needed for Rust in kernel space?
- Can we achieve Linux-like capabilities with significantly safer code?

## 📊 Current Status (February 2026)

| Component | Status | Notes |
|-----------|--------|-------|
| **Boot & MMU** | ✅ **Complete** | High-half addressing at `0xffffff8000000000`, EL initialization |
| **Memory Management** | 🔄 **Foundation** | Static 1GB block mappings, no dynamic allocator yet |
| **Exception Handling** | 📋 **Planned** | Basic panic handler (WFE loop), no interrupt vectors |
| **Device Drivers** | 📋 **Planned** | Hardcoded serial MMIO only |
| **Process Management** | 📋 **Future** | Single execution flow, no scheduler |
| **File Systems** | 📋 **Future** | No storage or file system support |
| **Networking** | 📋 **Future** | No network stack |

**Current Capabilities:**
- ✅ Boots on AArch64 from EL2 to EL1
- ✅ Configures MMU with proper cache attributes
- ✅ Operates in high-half virtual address space (Linux-compatible)
- ✅ Simple serial output via MMIO
- ✅ Multi-core filtering (primary core only)
- ✅ Clean separation of host vs bare-metal compilation

**Critical Limitations:**
- ❌ No dynamic memory allocation
- ❌ No interrupt or exception handling
- ❌ No concurrency or preemption
- ❌ No hardware abstraction layer
- ❌ No userspace/kernelspace separation

## 🏗️ Architecture Overview

### Design Philosophy
- **Monolithic design** similar to Linux kernel architecture
- **High-half kernel** at `0xffffff8000000000` (39-bit VA space)
- **Minimal `unsafe` code** - only for necessary hardware interactions
- **Clean separation** between architecture-specific and generic code
- **Research-first approach** - prioritizing safety and learnability over features

### Memory Layout
```
Kernel Virtual Base:   0xffffff8000000000
Load Offset:           0x80000
Kernel Virtual Start:  0xffffff8000080000
Page Size:             4KB (0x1000)
Stack Size:            64KB (0x10000)
```

### Boot Process
1. **Assembly bootstrap** (`boot.S`): EL initialization, CPU configuration
2. **MMU setup**: Translation tables with 1GB block mappings
3. **Virtual switch**: Jump to high-half address space
4. **BSS clearing**: Zero-initialize uninitialized data
5. **Kernel entry**: Call `kernel_main()` in Rust

## 🚀 Getting Started

### Prerequisites
```bash
# Install Rust toolchain
rustup target add aarch64-unknown-none

# Optional: Tools for inspection
cargo install cargo-binutils
rustup component add llvm-tools-preview

# For emulation (recommended)
brew install qemu  # macOS
# or apt-get install qemu-system-arm  # Ubuntu/Debian
```

### Building the Kernel
```bash
# For bare-metal AArch64 (primary target)
cargo build --target aarch64-unknown-none

# For development on host
cargo build

# Release build with optimizations
cargo build --target aarch64-unknown-none --release

# Check code without building
cargo check --target aarch64-unknown-none
```

### Running with QEMU
```bash
# Basic QEMU emulation
qemu-system-aarch64 \
  -machine virt \
  -cpu cortex-a57 \
  -kernel target/aarch64-unknown-none/debug/phoenix \
  -serial stdio \
  -nographic

# With more memory and SMP (when supported)
qemu-system-aarch64 \
  -machine virt \
  -cpu cortex-a57 \
  -smp 4 \
  -m 2G \
  -kernel target/aarch64-unknown-none/debug/phoenix \
  -serial stdio \
  -nographic
```

### Testing
```bash
# Run host-based tests
cargo test

# Run specific test
cargo test test_target_guard

# Verbose test output
cargo test -- --nocapture
```

## 📁 Project Structure
```
phoenix/
├── src/
│   ├── main.rs              # Kernel entry point with conditional compilation
│   ├── arch/               # Architecture-specific code
│   │   ├── mod.rs          # Architecture selector
│   │   └── aarch64/        # AArch64 implementation
│   │       ├── boot.S      # Assembly boot code (175 lines)
│   │       ├── kernel.ld   # Linker script (118 lines)
│   │       └── mod.rs      # AArch64 module with panic handler
│   └── (future: mm/, kernel/, drivers/, fs/, net/)
├── .cargo/
│   └── config.toml         # Target-specific rustflags
├── build.rs               # Build script tracking file changes
├── Cargo.toml            # Minimal package configuration
├── AGENTS.md             # AI agent development guidelines
└── README.md             # This file
```

## 🗺️ Development Roadmap

### Phase 1: Core Infrastructure (Q1 2026)
- [ ] **Dynamic memory allocator** (buddy system + slab allocator)
- [ ] **Exception vector table** and interrupt controller driver (GIC)
- [ ] **Timer subsystem** with ARM Generic Timer
- [ ] **Synchronization primitives** (spinlocks, mutexes, RCU)

### Phase 2: Process & Scheduling (Q2 2026)
- [ ] **Task management** with process control blocks
- [ ] **Scheduler** (CFS implementation)
- [ ] **System call interface** and user/kernel transition
- [ ] **Virtual memory manager** with page fault handling

### Phase 3: I/O & Storage (Q3 2026)
- [ ] **Device driver framework** with device tree parsing
- [ ] **Block device layer** and I/O scheduling
- [ ] **File system layer** (VFS with simple file systems)
- [ ] **Character device abstraction** (TTY, serial)

### Phase 4: Networking & Advanced Features (Q4 2026)
- [ ] **Network stack** (TCP/IP implementation)
- [ ] **Socket API** with BSD compatibility
- [ ] **Module system** for dynamic loading
- [ ] **Security subsystems** and debugging infrastructure

### Phase 5: Polish & Optimization (2027)
- [ ] **Performance optimization** and profiling
- [ ] **SMP support** for multi-core systems
- [ ] **Power management** features
- [ ] **Comprehensive testing** and validation

## 🤖 AI-Assisted Development

This project leverages **AI coding agents** through the [opencode](https://opencode.ai) platform for:

- **Architecture design** and subsystem planning
- **Code generation** with safety-focused patterns
- **Documentation** and specification writing
- **Testing strategy** development
- **Performance analysis** and optimization suggestions

**AI Contribution Guidelines:**
- All AI-generated code is reviewed and validated
- Safety annotations are required for `unsafe` blocks
- Code follows established Rust bare-metal patterns
- Architecture decisions are documented in AGENTS.md

## 🧪 Testing Strategy

| Test Type | Tools | Purpose |
|-----------|-------|---------|
| **Unit Tests** | `cargo test` | Host-based testing of Rust components |
| **Integration Tests** | QEMU | Bare-metal functionality validation |
| **Fuzz Testing** | `cargo-fuzz` | Security and stability testing |
| **Property Tests** | `proptest` | Invariant validation for core algorithms |
| **Benchmarks** | `criterion` | Performance monitoring and regression detection |

## 📚 Learning Resources

### Rust & Systems Programming
- [The Rust Programming Language](https://doc.rust-lang.org/book/)
- [The Rust Embedonomicon](https://docs.rust-embedded.org/embedonomicon/)
- [Writing an OS in Rust](https://os.phil-opp.com/)

### Kernel Development
- [Linux Kernel Documentation](https://www.kernel.org/doc/html/latest/)
- [ARM Architecture Reference Manual](https://developer.arm.com/documentation/ddi0487/latest)
- [Operating Systems: Three Easy Pieces](https://pages.cs.wisc.edu/~remzi/OSTEP/)

### Research Papers
- [Theseus: an Experiment in Operating System Structure](https://www.cs.jhu.edu/~huang/cs318/Spring21/Papers/theseus.pdf)
- [RedLeaf: Isolation and Communication in a Safe Operating System](https://unsafe-code.com/assets/papers/redleaf.pdf)

## 🤝 Contributing

Phoenix is a **research-oriented project** and welcomes contributions that align with our goals:

1. **Safety-first approach**: Prefer safe Rust patterns over `unsafe` code
2. **Documentation**: Code without documentation is incomplete
3. **Testing**: New features require corresponding tests
4. **Architecture alignment**: Follow Linux-like patterns when appropriate

**Getting Started:**
1. Read [AGENTS.md](AGENTS.md) for development guidelines
2. Check open issues for suitable tasks
3. Discuss major changes via GitHub issues first
4. Ensure code passes `cargo check --target aarch64-unknown-none`

## 📄 License

Licensed under the MIT License.

See [LICENSE](LICENSE) for the full license text.

## 🙏 Acknowledgments

- **Linux kernel developers** for architectural reference and inspiration
- **Rust embedded working group** for establishing bare-metal patterns
- **QEMU developers** for providing essential emulation capabilities
- **OpenCode.ai** for AI-assisted development infrastructure
- **Academic researchers** in safe systems programming whose work informs this project

## 📞 Contact & Discussion

- **GitHub Issues**: For bug reports and feature requests
- **Research Discussions**: Open GitHub discussions for architectural decisions
- **Development Updates**: Follow commit history for progress tracking

---

*"The most profound technologies are those that disappear. They weave themselves into the fabric of everyday life until they are indistinguishable from it."* - Mark Weiser

*Phoenix aims to make safe systems software so reliable it becomes invisible infrastructure.*