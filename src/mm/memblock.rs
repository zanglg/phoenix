//! Boot-time memory allocator similar to Linux memblock.
//!
//! This module provides a simple memory allocator used during kernel boot
//! before the full buddy system is initialized. It manages physical memory
//! regions with basic reserve and allocation operations.

use core::fmt;
use spin::Mutex;

/// Maximum number of memory regions that can be tracked.
const MAX_REGIONS: usize = 128;

/// A memory region descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    /// Starting physical address (inclusive).
    pub base: u64,
    /// Size of the region in bytes.
    pub size: u64,
    /// Region flags (reserved for future use).
    pub flags: u64,
}

impl Region {
    /// Creates a new region.
    pub const fn new(base: u64, size: u64) -> Self {
        Self {
            base,
            size,
            flags: 0,
        }
    }

    /// Returns the ending address (exclusive).
    pub fn end(&self) -> u64 {
        self.base + self.size
    }

    /// Checks if the region contains the given address.
    #[allow(dead_code)]
    pub fn contains(&self, addr: u64) -> bool {
        addr >= self.base && addr < self.end()
    }

    /// Checks if this region overlaps with another.
    pub fn overlaps(&self, other: &Region) -> bool {
        self.base < other.end() && other.base < self.end()
    }

    /// Checks if this region is adjacent to another (touching but not overlapping).
    #[allow(dead_code)]
    pub fn adjacent(&self, other: &Region) -> bool {
        self.end() == other.base || other.end() == self.base
    }
}

impl fmt::Display for Region {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "[{:#018x} - {:#018x}) ({:#x} bytes)",
            self.base,
            self.end(),
            self.size
        )
    }
}

/// The boot-time memory allocator.
#[derive(Debug)]
pub struct Memblock {
    /// Available memory regions.
    memory_regions: [Region; MAX_REGIONS],
    /// Number of valid entries in `memory_regions`.
    memory_count: usize,

    /// Reserved memory regions.
    reserved_regions: [Region; MAX_REGIONS],
    /// Number of valid entries in `reserved_regions`.
    reserved_count: usize,
}

impl Memblock {
    /// Creates a new empty memblock allocator.
    #[allow(dead_code)]
    pub const fn new() -> Self {
        Self {
            memory_regions: [Region::new(0, 0); MAX_REGIONS],
            memory_count: 0,
            reserved_regions: [Region::new(0, 0); MAX_REGIONS],
            reserved_count: 0,
        }
    }

    /// Adds a new memory region to the available pool.
    ///
    /// The region may be merged with existing adjacent regions.
    #[allow(dead_code)]
    pub fn add(&mut self, base: u64, size: u64) -> Result<(), &'static str> {
        if size == 0 {
            return Ok(());
        }

        let new_region = Region::new(base, size);

        // Check for overlap with existing memory regions
        for i in 0..self.memory_count {
            if self.memory_regions[i].overlaps(&new_region) {
                return Err("region overlaps with existing memory region");
            }
        }

        // Find insertion position (sorted by base address)
        let mut insert_pos = self.memory_count;
        for i in 0..self.memory_count {
            if self.memory_regions[i].base > new_region.base {
                insert_pos = i;
                break;
            }
        }

        // Shift regions to make space
        if self.memory_count >= MAX_REGIONS {
            return Err("maximum number of memory regions reached");
        }

        for i in (insert_pos..self.memory_count).rev() {
            self.memory_regions[i + 1] = self.memory_regions[i];
        }
        self.memory_regions[insert_pos] = new_region;
        self.memory_count += 1;

        // Merge adjacent regions
        self.merge_memory_regions();

        Ok(())
    }

    /// Reserves a region of memory (marks it as unavailable for allocation).
    #[allow(dead_code)]
    pub fn reserve(&mut self, base: u64, size: u64) -> Result<(), &'static str> {
        if size == 0 {
            return Ok(());
        }

        let new_reserved = Region::new(base, size);

        // Check for overlap with existing reserved regions
        for i in 0..self.reserved_count {
            if self.reserved_regions[i].overlaps(&new_reserved) {
                return Err("region overlaps with existing reserved region");
            }
        }

        // Find insertion position
        let mut insert_pos = self.reserved_count;
        for i in 0..self.reserved_count {
            if self.reserved_regions[i].base > new_reserved.base {
                insert_pos = i;
                break;
            }
        }

        if self.reserved_count >= MAX_REGIONS {
            return Err("maximum number of reserved regions reached");
        }

        for i in (insert_pos..self.reserved_count).rev() {
            self.reserved_regions[i + 1] = self.reserved_regions[i];
        }
        self.reserved_regions[insert_pos] = new_reserved;
        self.reserved_count += 1;

        // Merge adjacent reserved regions
        self.merge_reserved_regions();

        Ok(())
    }

    /// Removes a region from the available memory pool.
    ///
    /// This is used when memory becomes unavailable (e.g., device memory).
    #[allow(dead_code)]
    pub fn remove(&mut self, base: u64, size: u64) -> Result<(), &'static str> {
        if size == 0 {
            return Ok(());
        }

        let remove_region = Region::new(base, size);
        let mut new_memory = [Region::new(0, 0); MAX_REGIONS];
        let mut new_count = 0;

        for i in 0..self.memory_count {
            let region = self.memory_regions[i];

            if !region.overlaps(&remove_region) {
                // No overlap, keep region as is
                new_memory[new_count] = region;
                new_count += 1;
                continue;
            }

            // Region overlaps with removal area
            if remove_region.contains(region.base) && remove_region.contains(region.end() - 1) {
                // Entire region is removed
                continue;
            } else if remove_region.contains(region.base) {
                // Overlap at the beginning
                let new_base = remove_region.end();
                let new_size = region.end() - new_base;
                if new_size > 0 {
                    new_memory[new_count] = Region::new(new_base, new_size);
                    new_count += 1;
                }
            } else if remove_region.contains(region.end() - 1) {
                // Overlap at the end
                let new_size = remove_region.base - region.base;
                if new_size > 0 {
                    new_memory[new_count] = Region::new(region.base, new_size);
                    new_count += 1;
                }
            } else {
                // Removal area is in the middle
                let left_size = remove_region.base - region.base;
                let right_base = remove_region.end();
                let right_size = region.end() - right_base;

                if left_size > 0 {
                    new_memory[new_count] = Region::new(region.base, left_size);
                    new_count += 1;
                }
                if right_size > 0 {
                    new_memory[new_count] = Region::new(right_base, right_size);
                    new_count += 1;
                }
            }
        }

        self.memory_regions = new_memory;
        self.memory_count = new_count;

        Ok(())
    }

    /// Allocates a contiguous region of physical memory.
    ///
    /// Returns the base address of the allocated region, or an error if no
    /// suitable region could be found.
    #[allow(dead_code)]
    pub fn alloc(&mut self, size: u64, align: u64) -> Result<u64, &'static str> {
        if size == 0 {
            return Err("cannot allocate zero-sized region");
        }

        let align = align.max(1);

        // Find first fit in memory regions
        for i in 0..self.memory_count {
            let region = self.memory_regions[i];
            let mut aligned_base = (region.base + align - 1) & !(align - 1);

            while aligned_base + size <= region.end() {
                // Check if this candidate overlaps with any reserved region
                let candidate = Region::new(aligned_base, size);
                let mut overlaps = false;
                for j in 0..self.reserved_count {
                    if self.reserved_regions[j].overlaps(&candidate) {
                        overlaps = true;
                        break;
                    }
                }

                if !overlaps {
                    // Reserve this region
                    self.reserve(aligned_base, size)?;
                    return Ok(aligned_base);
                }

                // Try next aligned address
                aligned_base = (aligned_base + align) & !(align - 1);
                if aligned_base == 0 {
                    // Overflow, break
                    break;
                }
            }
        }

        Err("insufficient memory")
    }

    /// Returns the total size of all available memory regions.
    #[allow(dead_code)]
    pub fn total_memory(&self) -> u64 {
        let mut total = 0;
        for i in 0..self.memory_count {
            total += self.memory_regions[i].size;
        }
        total
    }

    /// Returns the total size of all reserved regions.
    #[allow(dead_code)]
    pub fn total_reserved(&self) -> u64 {
        let mut total = 0;
        for i in 0..self.reserved_count {
            total += self.reserved_regions[i].size;
        }
        total
    }

    /// Dumps the current state for debugging.
    #[allow(dead_code)]
    pub fn dump(&self) {
        // This function is a no-op by default.
        // To enable debug output, implementors should provide their own
        // output mechanism and uncomment the println! macros below.
        /*
        crate::println!("Memblock state:");
        crate::println!("  Memory regions ({}):", self.memory_count);
        for i in 0..self.memory_count {
            crate::println!("    {}", self.memory_regions[i]);
        }
        crate::println!("  Reserved regions ({}):", self.reserved_count);
        for i in 0..self.reserved_count {
            crate::println!("    {}", self.reserved_regions[i]);
        }
        crate::println!("  Total memory: {:#x}", self.total_memory());
        crate::println!("  Total reserved: {:#x}", self.total_reserved());
        */
    }

    /// Merges adjacent memory regions.
    #[allow(dead_code)]
    fn merge_memory_regions(&mut self) {
        if self.memory_count <= 1 {
            return;
        }

        let mut merged = [Region::new(0, 0); MAX_REGIONS];
        let mut merged_count = 1;
        merged[0] = self.memory_regions[0];

        for i in 1..self.memory_count {
            let current = self.memory_regions[i];
            let last = &mut merged[merged_count - 1];

            if last.adjacent(&current) {
                // Merge: extend the last region
                last.size += current.size;
            } else {
                merged[merged_count] = current;
                merged_count += 1;
            }
        }

        self.memory_regions = merged;
        self.memory_count = merged_count;
    }

    /// Merges adjacent reserved regions.
    #[allow(dead_code)]
    fn merge_reserved_regions(&mut self) {
        if self.reserved_count <= 1 {
            return;
        }

        let mut merged = [Region::new(0, 0); MAX_REGIONS];
        let mut merged_count = 1;
        merged[0] = self.reserved_regions[0];

        for i in 1..self.reserved_count {
            let current = self.reserved_regions[i];
            let last = &mut merged[merged_count - 1];

            if last.adjacent(&current) {
                last.size += current.size;
            } else {
                merged[merged_count] = current;
                merged_count += 1;
            }
        }

        self.reserved_regions = merged;
        self.reserved_count = merged_count;
    }
}

/// Global instance of the memblock allocator.
#[allow(dead_code)]
static MEMBLOCK: Mutex<Memblock> = Mutex::new(Memblock::new());

/// Returns a lock guard for the global memblock instance.
///
/// This function provides safe concurrent access to the memblock allocator.
#[allow(dead_code)]
pub fn lock() -> spin::MutexGuard<'static, Memblock> {
    MEMBLOCK.lock()
}

/// Initializes the memblock allocator with the given memory region.
///
/// This should be called early during kernel boot.
#[allow(dead_code)]
pub fn init(base: u64, size: u64) -> Result<(), &'static str> {
    let mut mb = lock();
    mb.add(base, size)
}

/// Reserves a region of memory.
#[allow(dead_code)]
pub fn reserve(base: u64, size: u64) -> Result<(), &'static str> {
    let mut mb = lock();
    mb.reserve(base, size)
}

/// Allocates a contiguous region of physical memory.
#[allow(dead_code)]
pub fn alloc(size: u64, align: u64) -> Result<u64, &'static str> {
    let mut mb = lock();
    mb.alloc(size, align)
}

#[cfg(all(test, not(target_os = "none")))]
mod tests {
    use super::*;

    #[test]
    fn test_region_contains() {
        let region = Region::new(0x1000, 0x1000);
        assert!(region.contains(0x1000));
        assert!(region.contains(0x1fff));
        assert!(!region.contains(0x2000));
        assert!(!region.contains(0xfff));
    }

    #[test]
    fn test_region_overlaps() {
        let r1 = Region::new(0x1000, 0x1000);
        let r2 = Region::new(0x1800, 0x1000);
        let r3 = Region::new(0x2000, 0x1000);
        assert!(r1.overlaps(&r2));
        assert!(r2.overlaps(&r1));
        assert!(!r1.overlaps(&r3));
        assert!(!r3.overlaps(&r1));
    }

    #[test]
    fn test_region_adjacent() {
        let r1 = Region::new(0x1000, 0x1000);
        let r2 = Region::new(0x3000, 0x1000);
        let r3 = Region::new(0x1800, 0x1000);
        assert!(!r1.adjacent(&r2)); // not touching
        assert!(!r1.adjacent(&r3)); // overlapping
        let r4 = Region::new(0x2000, 0x1000);
        assert!(r1.adjacent(&r4)); // r1 ends at 0x2000, r4 starts at 0x2000
    }

    #[test]
    fn test_memblock_add() {
        let mut mb = Memblock::new();
        assert!(mb.add(0x1000, 0x1000).is_ok());
        assert_eq!(mb.memory_count, 1);
        assert_eq!(mb.total_memory(), 0x1000);

        // Adding overlapping region should fail
        assert!(mb.add(0x1800, 0x1000).is_err());

        // Adding adjacent region should merge
        assert!(mb.add(0x2000, 0x1000).is_ok());
        assert_eq!(mb.memory_count, 1); // merged
        assert_eq!(mb.total_memory(), 0x2000);
    }

    #[test]
    fn test_memblock_reserve() {
        let mut mb = Memblock::new();
        mb.add(0x1000, 0x1000).unwrap();
        mb.reserve(0x1200, 0x200).unwrap();
        assert_eq!(mb.reserved_count, 1);
        assert_eq!(mb.total_reserved(), 0x200);
    }

    #[test]
    fn test_memblock_alloc() {
        let mut mb = Memblock::new();
        mb.add(0x1000, 0x1000).unwrap();
        mb.reserve(0x1200, 0x200).unwrap();

        // Allocate within available space
        let addr = mb.alloc(0x100, 0x10).unwrap();
        assert!(addr >= 0x1000 && addr + 0x100 <= 0x2000);
        // Should be reserved now
        assert_eq!(mb.reserved_count, 2);

        // Add a non-adjacent memory region
        mb.add(0x3000, 0x1000).unwrap();
        // Allocate 0x1000 bytes, should go into the new region
        let addr2 = mb.alloc(0x1000, 0x1).unwrap();
        assert!(addr2 >= 0x3000 && addr2 + 0x1000 <= 0x4000);
    }

    #[test]
    fn test_memblock_remove() {
        let mut mb = Memblock::new();
        mb.add(0x1000, 0x1000).unwrap();
        mb.remove(0x1800, 0x400).unwrap();
        assert_eq!(mb.memory_count, 2); // split into two regions
        assert_eq!(mb.total_memory(), 0xc00); // 0x1000 - 0x400
    }

    #[test]
    fn test_memblock_merge() {
        let mut mb = Memblock::new();
        mb.add(0x1000, 0x1000).unwrap();
        mb.add(0x3000, 0x1000).unwrap();
        // Not adjacent, so two regions
        assert_eq!(mb.memory_count, 2);

        let mut mb2 = Memblock::new();
        mb2.add(0x1000, 0x1000).unwrap();
        mb2.add(0x3000, 0x1000).unwrap();
        // Now add adjacent region that bridges the gap
        mb2.add(0x2000, 0x1000).unwrap();
        // Should merge into one region (all three are adjacent)
        assert_eq!(mb2.memory_count, 1);
        assert_eq!(mb2.total_memory(), 0x3000);
    }
}
