//! Explicit `phoenix.system-stress` boot diagnostics, before the first EL0 entry.
//! No fault injection is reachable through the native syscall interface.
use super::*;
use phoenix_kernel::arch::aarch64::paging::Descriptor;
use phoenix_kernel::arch::aarch64::user_page_table::TranslationTableMemory;
use phoenix_kernel::memory::{FrameRange, PAGE_SIZE, PageFrame};
use phoenix_kernel::process_image::ProcessImageMemory;

struct Inject<'a, M> {
    memory: &'a mut M,
    fail_at: Option<usize>,
    calls: usize,
}
impl<M> Inject<'_, M> {
    fn operation(&mut self) -> Result<(), ()> {
        let index = self.calls;
        self.calls += 1;
        if self.fail_at == Some(index) {
            Err(())
        } else {
            Ok(())
        }
    }
}
impl<M: ProcessImageMemory> ProcessImageMemory for Inject<'_, M> {
    type Error = ();
    fn clear_frame(&mut self, frame: PageFrame) -> Result<(), ()> {
        self.operation()?;
        self.memory.clear_frame(frame).map_err(|_| ())
    }
    fn write_frame(&mut self, frame: PageFrame, offset: usize, bytes: &[u8]) -> Result<(), ()> {
        self.operation()?;
        self.memory
            .write_frame(frame, offset, bytes)
            .map_err(|_| ())
    }
}
impl<M: TranslationTableMemory> TranslationTableMemory for Inject<'_, M> {
    type Error = ();
    fn clear_table(&mut self, frame: PageFrame) -> Result<(), ()> {
        self.operation()?;
        self.memory.clear_table(frame).map_err(|_| ())
    }
    fn write_descriptor(
        &mut self,
        frame: PageFrame,
        index: usize,
        descriptor: Descriptor,
    ) -> Result<(), ()> {
        self.operation()?;
        self.memory
            .write_descriptor(frame, index, descriptor)
            .map_err(|_| ())
    }
}

pub(super) fn run(runtime: &mut State, console: &mut Console<EarlyPl011>) {
    assert!(runtime.test && irqs_masked() && runtime.parent.is_none());
    let before = runtime.allocator;
    let mut memory = memory(runtime);
    // Withhold all but each incomplete frame budget from the real allocator.
    // The saved free set owns only unused frames. IRQ masking and absence of EL0
    // execution exclude other allocators. No image/table owner escapes a failed
    // load, so returning the withheld frames by restoring the snapshot is safe.
    let required = INIT_IMAGE_PAGES + INIT_TABLE_PAGES;
    assert!(before.free_ranges()[0].len() >= required);
    for available in 0..required {
        for range in before.free_ranges() {
            assert_eq!(
                runtime
                    .allocator
                    .allocate_contiguous(range.len(), 1)
                    .unwrap(),
                Some(*range)
            );
        }
        if available != 0 {
            let start = before.free_ranges()[0].start_frame_number() * PAGE_SIZE;
            let first = PageFrame::from_start(PhysAddr::new(start)).unwrap();
            runtime
                .allocator
                .deallocate_contiguous(FrameRange::new(first, available).unwrap())
                .unwrap();
        }
        let limited = runtime.allocator;
        assert!(matches!(
            load(
                runtime.archive,
                "child",
                &mut runtime.allocator,
                &mut memory
            ),
            Err(Errno::NoMemory)
        ));
        assert_eq!(runtime.allocator, limited, "partial allocation rollback");
        runtime.allocator = before;
    }

    let operations = {
        let mut injected = Inject {
            memory: &mut memory,
            fail_at: None,
            calls: 0,
        };
        let image = load(
            runtime.archive,
            "child",
            &mut runtime.allocator,
            &mut injected,
        )
        .unwrap();
        image.release_unpublished(&mut runtime.allocator).unwrap();
        injected.calls
    };
    assert_eq!(runtime.allocator, before);
    // Fail every clear/copy/table-write boundary after preceding operations have
    // really touched private target frames; require exact free-set rollback.
    for index in 0..operations {
        let mut injected = Inject {
            memory: &mut memory,
            fail_at: Some(index),
            calls: 0,
        };
        assert!(matches!(
            load(
                runtime.archive,
                "child",
                &mut runtime.allocator,
                &mut injected
            ),
            Err(Errno::InputOutput)
        ));
        assert_eq!(injected.calls, index + 1);
        assert_eq!(runtime.allocator, before, "population rollback");
    }

    let mut held = [None; 255];
    let mut count = 0;
    while let Ok(asid) = runtime.asids.allocate() {
        held[count] = Some(asid);
        count += 1;
    }
    assert_eq!(count, usize::from(runtime.asids_without_child));
    let mut frame = ExceptionFrame::zeroed();
    let parent = runtime.current;
    assert_eq!(
        spawn_child(runtime, &mut frame, "child"),
        Err(Errno::NoMemory)
    );
    assert_eq!(frame, ExceptionFrame::zeroed());
    assert_eq!(
        runtime.allocator, before,
        "ASID exhaustion leaked loaded frames"
    );
    assert_eq!(runtime.current, parent);
    assert!(runtime.parent.is_none());
    assert_eq!(runtime.processes.len(), 1);
    for asid in held.into_iter().flatten() {
        // SAFETY: These diagnostic reservations never had a root installed and
        // are uniquely held here. The init ASID was never included in `held`.
        unsafe { runtime.asids.release_retired(asid) }.unwrap();
    }
    assert_eq!(runtime.asids.remaining(), runtime.asids_without_child);
    // A clean load immediately after all failures must still populate and retire.
    let retry = load(
        runtime.archive,
        "child",
        &mut runtime.allocator,
        &mut memory,
    )
    .unwrap();
    retry.release_unpublished(&mut runtime.allocator).unwrap();
    assert_eq!(runtime.allocator, before);
    let _ = writeln!(
        console,
        "PHOENIX_RECOVERY frames={required} backend={operations} asids={count} retry=ok"
    );
}
