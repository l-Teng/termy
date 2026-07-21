import AppKit
import CTermy
import Foundation

enum LibTermyError: Error, CustomStringConvertible {
    case missingTerminal
    case missingCells
    case missingGraphicsPlacements
    case cellCountMismatch

    var description: String {
        switch self {
        case .missingTerminal:
            return "libtermy did not return a terminal handle"
        case .missingCells:
            return "libtermy returned a frame without cells"
        case .missingGraphicsPlacements:
            return "libtermy returned graphics metadata without placements"
        case .cellCountMismatch:
            return "libtermy returned a cell count that does not match the frame damage"
        }
    }
}

private final class TerminalWakeupMonitor: @unchecked Sendable {
    private let handle: OpaquePointer
    private let onWakeup: @MainActor () -> Void
    private let queue = DispatchQueue(label: "dev.termy.terminal-wakeup", qos: .userInteractive)
    private let group = DispatchGroup()
    private let lock = NSLock()
    private var running = true

    init(handle: OpaquePointer, onWakeup: @escaping @MainActor () -> Void) {
        self.handle = handle
        self.onWakeup = onWakeup
        group.enter()
        queue.async { [weak self] in
            self?.run()
        }
    }

    func stop() {
        lock.lock()
        let wasRunning = running
        running = false
        lock.unlock()

        guard wasRunning else {
            return
        }

        _ = termy_terminal_notify_wakeup(handle)
        group.wait()
    }

    private func run() {
        defer {
            group.leave()
        }

        while isRunning {
            var woke = false
            // The timeout is a safety net only: PTY output and `stop()` (via
            // `termy_terminal_notify_wakeup`) both wake the blocking wait
            // immediately, so a long timeout keeps the monitor thread asleep
            // instead of waking 4×/s per pane just to re-check `running`.
            let status = termy_terminal_wait_for_wakeup(handle, 10_000, &woke)
            guard status == TERMY_FFI_OK else {
                return
            }
            guard woke, isRunning else {
                continue
            }
            Task { @MainActor [weak self] in
                guard let self, self.isRunning else {
                    return
                }
                self.onWakeup()
            }
        }
    }

    private var isRunning: Bool {
        lock.lock()
        defer {
            lock.unlock()
        }
        return running
    }
}

final class LibTermyTerminal {
    private var handle: OpaquePointer?
    private var configHandle: OpaquePointer?
    private var wakeupMonitor: TerminalWakeupMonitor?

    let renderConfig: TerminalRenderConfig

    init(
        cols: UInt16 = 96,
        rows: UInt16 = 28,
        loadUserConfig: Bool = true,
        workingDirectoryOverride: String? = nil,
        startupCommand: String? = nil
    ) throws {
        var size = termy_size_default()
        size.cols = cols
        size.rows = rows
        let config = try loadUserConfig ? Self.loadDefaultConfig() : nil
        let systemAppearance = config.map { _ in Self.currentSystemAppearanceRawValue() }
        // `renderConfig` (a non-optional `let`) is the last property to gain a
        // value, so until it's set the object is not fully initialized and
        // `deinit` won't run. If `renderConfig(for:)` throws after we've stored
        // `config`, the boxed config would leak — so free it explicitly here.
        do {
            renderConfig = try Self.renderConfig(
                for: config,
                systemAppearanceRawValue: systemAppearance
            )
        } catch {
            if let config {
                _ = termy_config_free(config)
            }
            throw error
        }
        configHandle = config
        let workingDirectory: String?
        if let override = Self.normalizedWorkingDirectory(workingDirectoryOverride) {
            workingDirectory = override
        } else {
            workingDirectory = try Self.workingDirectory(for: config)
        }

        var terminal: OpaquePointer?
        let workingDirectoryBytes = workingDirectory.map { Array($0.utf8) } ?? []
        let startupCommandBytes = startupCommand.map { Array($0.utf8) } ?? []
        let status = workingDirectoryBytes.withUnsafeBufferPointer { workingDirectoryBuffer in
            startupCommandBytes.withUnsafeBufferPointer { startupCommandBuffer in
                var options = TermyFfiTerminalOptions(
                    config: config,
                    working_directory_ptr: workingDirectoryBuffer.baseAddress,
                    working_directory_len: workingDirectoryBuffer.count,
                    startup_command_ptr: startupCommandBuffer.baseAddress,
                    startup_command_len: startupCommandBuffer.count,
                    env_vars_ptr: nil,
                    env_vars_len: 0
                )
                return termy_terminal_new_with_options(size, &options, &terminal)
            }
        }
        try TermyFfiBridge.requireOK("termy_terminal_new_with_options", status)

        guard let terminal else {
            throw LibTermyError.missingTerminal
        }
        do {
            try Self.applyConfigColors(
                to: terminal,
                from: config,
                systemAppearanceRawValue: systemAppearance
            )
        } catch {
            _ = termy_terminal_free(terminal)
            if let configHandle {
                _ = termy_config_free(configHandle)
                self.configHandle = nil
            }
            throw error
        }
        // The Swift shell consumes the FFI wake channel, so plain wakeups are
        // useful: they move idle terminals from timer cadence to immediate poll.
        _ = termy_terminal_set_wakeup_enabled(terminal, true)
        handle = terminal
    }

    /// Creates a display-only terminal (no shell/PTY) for rendering tmux
    /// control-mode pane output. Drive it with `feedOutput`; `write` is a no-op.
    init(displayCols cols: UInt16, rows: UInt16, loadUserConfig: Bool = true) throws {
        var size = termy_size_default()
        size.cols = cols
        size.rows = rows
        let config = try loadUserConfig ? Self.loadDefaultConfig() : nil
        let systemAppearance = config.map { _ in Self.currentSystemAppearanceRawValue() }
        // See the main initializer: free `config` if `renderConfig(for:)` throws
        // before the object is fully initialized, since `deinit` won't run yet.
        do {
            renderConfig = try Self.renderConfig(
                for: config,
                systemAppearanceRawValue: systemAppearance
            )
        } catch {
            if let config {
                _ = termy_config_free(config)
            }
            throw error
        }
        configHandle = config

        var terminal: OpaquePointer?
        try TermyFfiBridge.requireOK(
            "termy_display_terminal_new",
            termy_display_terminal_new(size, &terminal)
        )
        guard let terminal else {
            throw LibTermyError.missingTerminal
        }
        do {
            try Self.applyConfigColors(
                to: terminal,
                from: config,
                systemAppearanceRawValue: systemAppearance
            )
        } catch {
            _ = termy_terminal_free(terminal)
            if let configHandle {
                _ = termy_config_free(configHandle)
                self.configHandle = nil
            }
            throw error
        }
        handle = terminal
    }

    deinit {
        stopWakeupMonitor()
        if let handle {
            _ = termy_terminal_free(handle)
        }
        if let configHandle {
            _ = termy_config_free(configHandle)
        }
    }

    func startWakeupMonitor(onWakeup: @escaping @MainActor () -> Void) {
        stopWakeupMonitor()
        guard let handle else {
            return
        }
        wakeupMonitor = TerminalWakeupMonitor(handle: handle, onWakeup: onWakeup)
    }

    func stopWakeupMonitor() {
        wakeupMonitor?.stop()
        wakeupMonitor = nil
    }

    /// Advance the grid with output bytes (e.g. tmux `%output`) on a display
    /// terminal, without sending input to a PTY.
    func feedOutput(_ bytes: [UInt8]) throws {
        let handle = try terminalHandle()
        let status = bytes.withUnsafeBufferPointer { buffer in
            termy_terminal_feed_output(handle, buffer.baseAddress, buffer.count)
        }
        try TermyFfiBridge.requireOK("termy_terminal_feed_output", status)
    }

    func write(_ bytes: [UInt8]) throws {
        let handle = try terminalHandle()
        let status = bytes.withUnsafeBufferPointer { buffer in
            termy_terminal_write(handle, buffer.baseAddress, buffer.count)
        }
        try TermyFfiBridge.requireOK("termy_terminal_write", status)
    }

    func encodeKey(
        _ keyInput: TerminalKeyInput,
        macosOptionAsAlt: Bool = false
    ) throws -> [UInt8]? {
        let handle = try terminalHandle()

        let keyBytes = Array(keyInput.key.utf8)
        let keyCharBytes = keyInput.keyChar.map { Array($0.utf8) } ?? []
        var outBytes = TermyFfiBytes()

        let status = keyBytes.withUnsafeBufferPointer { keyBuffer in
            keyCharBytes.withUnsafeBufferPointer { keyCharBuffer in
                var ffiKeystroke = TermyFfiKeystroke(
                    control: keyInput.control,
                    alt: keyInput.alt,
                    shift: keyInput.shift,
                    platform: keyInput.platform,
                    function: keyInput.function,
                    key_ptr: keyBuffer.baseAddress,
                    key_len: keyBuffer.count,
                    key_char_ptr: keyCharBuffer.baseAddress,
                    key_char_len: keyCharBuffer.count,
                    event_kind: keyInput.eventKind.rawValue
                )
                return termy_terminal_encode_key_with_options(
                    handle,
                    &ffiKeystroke,
                    macosOptionAsAlt,
                    &outBytes
                )
            }
        }
        try TermyFfiBridge.requireOK("termy_terminal_encode_key_with_options", status)
        defer {
            if outBytes.ptr != nil {
                _ = termy_buffer_free(outBytes)
            }
        }

        guard let ptr = outBytes.ptr, outBytes.len > 0 else {
            return nil
        }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(outBytes.len)))
    }

    func encodeMouse(_ mouseInput: TerminalMouseInput) throws -> [UInt8]? {
        let handle = try terminalHandle()
        var ffiInput = TermyFfiMouseInput(
            kind: mouseInput.kind.rawValue,
            button: mouseInput.button.rawValue,
            col: mouseInput.position.col,
            row: mouseInput.position.row,
            control: mouseInput.control,
            alt: mouseInput.alt,
            shift: mouseInput.shift
        )
        var outBytes = TermyFfiBytes()
        try TermyFfiBridge.requireOK(
            "termy_terminal_encode_mouse",
            termy_terminal_encode_mouse(handle, &ffiInput, &outBytes)
        )
        defer {
            if outBytes.ptr != nil {
                _ = termy_buffer_free(outBytes)
            }
        }

        guard let ptr = outBytes.ptr, outBytes.len > 0 else {
            return nil
        }
        return Array(UnsafeBufferPointer(start: ptr, count: Int(outBytes.len)))
    }

    func resize(cols: UInt16, rows: UInt16, cellWidth: Float, cellHeight: Float) throws {
        let handle = try terminalHandle()
        var size = termy_size_default()
        size.cols = cols
        size.rows = rows
        size.cell_width = cellWidth
        size.cell_height = cellHeight
        try TermyFfiBridge.requireOK("termy_terminal_resize", termy_terminal_resize(handle, size))
    }

    func scrollDisplay(deltaLines: Int32) throws -> Bool {
        try changedBy("termy_terminal_scroll_display") { handle, changed in
            termy_terminal_scroll_display(handle, deltaLines, changed)
        }
    }

    func scrollToBottom() throws -> Bool {
        try changedBy("termy_terminal_scroll_to_bottom") { handle, changed in
            termy_terminal_scroll_to_bottom(handle, changed)
        }
    }

    /// Reload render settings and the live terminal palette from one config
    /// snapshot and one effective AppKit appearance.
    func reloadAppearance() throws -> TerminalRenderConfig {
        let handle = try terminalHandle()
        let config = try Self.loadDefaultConfig()
        defer {
            if let config {
                _ = termy_config_free(config)
            }
        }
        let systemAppearance = Self.currentSystemAppearanceRawValue()
        let renderConfig = try Self.renderConfig(
            for: config,
            systemAppearanceRawValue: systemAppearance
        )
        try Self.applyConfigColors(
            to: handle,
            from: config,
            systemAppearanceRawValue: systemAppearance
        )
        return renderConfig
    }

    /// Load a fresh render config (font, metrics, colors, padding) from the
    /// on-disk config without touching any running terminal.
    static func loadRenderConfig() throws -> TerminalRenderConfig {
        let config = try loadDefaultConfig()
        defer {
            if let config {
                _ = termy_config_free(config)
            }
        }
        return try renderConfig(
            for: config,
            systemAppearanceRawValue: currentSystemAppearanceRawValue()
        )
    }

    func clearScrollback() throws -> Bool {
        try changedBy("termy_terminal_clear_scrollback") { handle, changed in
            termy_terminal_clear_scrollback(handle, changed)
        }
    }

    /// Whether the foreground program has enabled bracketed-paste mode. When
    /// true, the host wraps pasted text in `ESC[200~`/`ESC[201~` so the program
    /// can treat it as inert data rather than typed input.
    func bracketedPasteMode() throws -> Bool {
        try changedBy("termy_terminal_bracketed_paste_mode") { handle, enabled in
            termy_terminal_bracketed_paste_mode(handle, enabled)
        }
    }

    func setScrollbackHistory(_ scrollbackHistory: Int) throws {
        let handle = try terminalHandle()
        try TermyFfiBridge.requireOK(
            "termy_terminal_set_scrollback_history",
            termy_terminal_set_scrollback_history(handle, max(0, scrollbackHistory))
        )
    }

    func drainEvents() throws -> [TerminalRuntimeEvent] {
        let handle = try terminalHandle()
        var batch = TermyFfiEventBatch()
        try TermyFfiBridge.requireOK(
            "termy_terminal_drain_events",
            termy_terminal_drain_events(handle, &batch)
        )
        defer {
            _ = termy_event_batch_free(&batch)
        }

        guard let eventsPtr = batch.events_ptr else {
            return []
        }
        return UnsafeBufferPointer(start: eventsPtr, count: Int(batch.events_len))
            .compactMap(Self.event(from:))
    }

    func snapshot() throws -> TerminalFrame {
        let handle = try terminalHandle()

        var frame = TermyFfiFrame()
        try TermyFfiBridge.requireOK("termy_terminal_snapshot", termy_terminal_snapshot(handle, &frame))
        defer {
            _ = termy_frame_free(&frame)
        }

        guard let cellsPtr = frame.cells_ptr else {
            throw LibTermyError.missingCells
        }

        // Full frames are row-major; positions are derived from the index.
        let cols = Int(frame.cols)
        guard cols > 0, Int(frame.cells_len) == cols * Int(frame.rows) else {
            throw LibTermyError.cellCountMismatch
        }
        let cells = UnsafeBufferPointer(start: cellsPtr, count: Int(frame.cells_len))
            .enumerated()
            .map { index, ffiCell in
                Self.cell(from: ffiCell, col: index % cols, row: index / cols)
            }
        let cursor = frame.cursor.visible
            ? TerminalCursor(
                col: Int(frame.cursor.col),
                row: Int(frame.cursor.row),
                style: TerminalCursorStyle(ffiRawValue: frame.cursor.style)
            )
            : nil

        return TerminalFrame(
            cols: Int(frame.cols),
            rows: Int(frame.rows),
            cells: cells,
            cursor: cursor,
            displayOffset: Int(frame.display_offset),
            historySize: Int(frame.history_size)
        )
    }

    func frameUpdate(forceFull: Bool) throws -> TerminalFrameUpdate {
        let handle = try terminalHandle()

        var update = TermyFfiFrameUpdate()
        try TermyFfiBridge.requireOK(
            "termy_terminal_take_frame_update",
            termy_terminal_take_frame_update(handle, forceFull, &update)
        )
        defer {
            _ = termy_frame_update_free(&update)
        }

        let damage: TerminalDamage
        switch update.damage_kind {
        case 1:
            damage = .full
        case 2:
            if update.spans_len > 0 {
                guard let spansPtr = update.spans_ptr else {
                    damage = .none
                    break
                }
                let spans = UnsafeBufferPointer(start: spansPtr, count: Int(update.spans_len))
                    .map {
                        TerminalDirtySpan(
                            row: Int($0.row),
                            leftCol: Int($0.left_col),
                            rightCol: Int($0.right_col)
                        )
                    }
                damage = .partial(spans)
            } else {
                damage = .none
            }
        default:
            damage = .none
        }

        // Cells carry no position over the FFI: a full update is row-major and
        // a partial update lists cells in dirty-span order.
        let cells: [TerminalCell]
        if update.cells_len > 0 {
            guard let cellsPtr = update.cells_ptr else {
                throw LibTermyError.missingCells
            }
            let ffiCells = UnsafeBufferPointer(start: cellsPtr, count: Int(update.cells_len))
            switch damage {
            case .full:
                let cols = Int(update.cols)
                guard cols > 0, ffiCells.count == cols * Int(update.rows) else {
                    throw LibTermyError.cellCountMismatch
                }
                cells = ffiCells.enumerated().map { index, ffiCell in
                    Self.cell(from: ffiCell, col: index % cols, row: index / cols)
                }
            case .partial(let spans):
                guard spans.allSatisfy({ $0.leftCol <= $0.rightCol }),
                      ffiCells.count == spans.reduce(0, { $0 + ($1.rightCol - $1.leftCol + 1) })
                else {
                    throw LibTermyError.cellCountMismatch
                }
                var result: [TerminalCell] = []
                result.reserveCapacity(ffiCells.count)
                var index = 0
                for span in spans {
                    for col in span.leftCol...span.rightCol {
                        result.append(Self.cell(from: ffiCells[index], col: col, row: span.row))
                        index += 1
                    }
                }
                cells = result
            case .none:
                throw LibTermyError.cellCountMismatch
            }
        } else {
            cells = []
        }

        return TerminalFrameUpdate(
            cols: Int(update.cols),
            rows: Int(update.rows),
            cells: cells,
            cursor: Self.cursor(from: update.cursor),
            displayOffset: Int(update.display_offset),
            historySize: Int(update.history_size),
            damage: damage
        )
    }

    func kittyGraphicsRevision() throws -> UInt64 {
        let handle = try terminalHandle()
        var revision: UInt64 = 0
        try TermyFfiBridge.requireOK(
            "termy_terminal_kitty_graphics_revision",
            termy_terminal_kitty_graphics_revision(handle, &revision)
        )
        return revision
    }

    func kittyGraphicsPlacements() throws -> TerminalKittyGraphicsSnapshot {
        let handle = try terminalHandle()
        var batch = TermyFfiKittyGraphicsBatch()
        try TermyFfiBridge.requireOK(
            "termy_terminal_kitty_graphics_placements",
            termy_terminal_kitty_graphics_placements(handle, &batch)
        )
        defer {
            _ = termy_kitty_graphics_batch_free(&batch)
        }

        guard batch.placements_len > 0 else {
            return TerminalKittyGraphicsSnapshot(revision: batch.revision, placements: [])
        }
        guard let placementsPtr = batch.placements_ptr else {
            throw LibTermyError.missingGraphicsPlacements
        }

        let placements = UnsafeBufferPointer(
            start: placementsPtr,
            count: Int(batch.placements_len)
        ).map { placement in
            let png: Data
            if placement.png.len > 0, let pngPtr = placement.png.ptr {
                png = Data(bytes: pngPtr, count: Int(placement.png.len))
            } else {
                png = Data()
            }
            return TerminalKittyGraphicsPlacement(
                placementSerial: placement.placement_serial,
                imageID: placement.image_id,
                placementID: placement.placement_id,
                png: png,
                imageWidth: Int(placement.image_width),
                imageHeight: Int(placement.image_height),
                imageGeneration: placement.image_generation,
                viewportRow: Int(placement.viewport_row),
                col: Int(placement.col),
                sourceX: Int(placement.source_x),
                sourceY: Int(placement.source_y),
                sourceWidth: Int(placement.source_width),
                sourceHeight: Int(placement.source_height),
                displayCols: placement.has_display_cols ? Int(placement.display_cols) : nil,
                displayRows: placement.has_display_rows ? Int(placement.display_rows) : nil,
                occupiedCols: Int(placement.occupied_cols),
                occupiedRows: Int(placement.occupied_rows),
                xOffset: Int(placement.x_offset),
                yOffset: Int(placement.y_offset),
                zIndex: Int(placement.z_index)
            )
        }
        .sorted(by: TerminalKittyGraphicsPlacement.paintOrder)

        return TerminalKittyGraphicsSnapshot(
            revision: batch.revision,
            placements: placements
        )
    }

    /// The OSC 8 hyperlink under a viewport cell, if any, expanded to the
    /// contiguous link run on that row. Best-effort: returns nil on FFI errors
    /// since hover lookups should never surface failures.
    func hyperlink(atRow row: Int, col: Int) -> TerminalFrameLink? {
        guard row >= 0, col >= 0, let handle else {
            return nil
        }

        var found = false
        var link = TermyFfiHyperlink()
        guard termy_terminal_hyperlink_at(handle, row, col, &found, &link) == TERMY_FFI_OK,
              found
        else {
            return nil
        }
        defer { _ = termy_hyperlink_free(&link) }

        guard let target = TermyFfiBridge.string(from: link.uri), !target.isEmpty else {
            return nil
        }
        return TerminalFrameLink(
            row: row,
            startCol: Int(link.start_col),
            endCol: Int(link.end_col),
            target: target
        )
    }

    func search(
        _ query: String,
        options: TerminalSearchOptions = TerminalSearchOptions()
    ) throws -> [TerminalSearchMatch] {
        let handle = try terminalHandle()

        var batch = TermyFfiSearchBatch()
        let ffiOptions = TermyFfiSearchOptions(
            case_sensitive: options.caseSensitive,
            regex: options.usesRegex
        )
        let status = Array(query.utf8).withUnsafeBufferPointer { buffer in
            termy_terminal_search_with_options(
                handle,
                buffer.baseAddress,
                buffer.count,
                ffiOptions,
                &batch
            )
        }
        try TermyFfiBridge.requireOK("termy_terminal_search_with_options", status)
        defer {
            _ = termy_search_batch_free(&batch)
        }

        guard let matchesPtr = batch.matches_ptr else {
            return []
        }
        return UnsafeBufferPointer(start: matchesPtr, count: Int(batch.matches_len))
            .map { match in
                TerminalSearchMatch(
                    row: Int(match.row),
                    startCol: Int(match.start_col),
                    endCol: Int(match.end_col)
                )
            }
    }

    private func terminalHandle() throws -> OpaquePointer {
        guard let handle else {
            throw LibTermyError.missingTerminal
        }
        return handle
    }

    private func changedBy(
        _ operation: String,
        _ call: (OpaquePointer, UnsafeMutablePointer<Bool>) -> TermyFfiStatus
    ) throws -> Bool {
        let handle = try terminalHandle()
        var changed = false
        try TermyFfiBridge.requireOK(operation, call(handle, &changed))
        return changed
    }

    private static func loadDefaultConfig() throws -> OpaquePointer? {
        var config: OpaquePointer?
        try TermyFfiBridge.requireOK("termy_config_load_default", termy_config_load_default(&config))
        return config
    }

    private static func renderConfig(
        for config: OpaquePointer?,
        systemAppearanceRawValue: UInt32?
    ) throws -> TerminalRenderConfig {
        guard let config else {
            return .default
        }

        var renderConfig = TermyFfiRenderConfig()
        try TermyFfiBridge.requireOK(
            "termy_config_render_config_for_appearance",
            termy_config_render_config_for_appearance(
                config,
                systemAppearanceRawValue ?? 1,
                &renderConfig
            )
        )
        defer {
            _ = termy_render_config_free(&renderConfig)
        }
        return TerminalRenderConfig(renderConfig)
    }

    private static func applyConfigColors(
        to terminal: OpaquePointer,
        from config: OpaquePointer?,
        systemAppearanceRawValue: UInt32?
    ) throws {
        guard let config, let systemAppearanceRawValue else {
            return
        }
        try TermyFfiBridge.requireOK(
            "termy_terminal_apply_config_colors_for_appearance",
            termy_terminal_apply_config_colors_for_appearance(
                terminal,
                config,
                systemAppearanceRawValue
            )
        )
    }

    private static func currentSystemAppearanceRawValue() -> UInt32 {
        // User-configured terminals are owned by AppKit/SwiftUI main-actor
        // models. Resolve the effective app appearance there so per-app and
        // accessibility appearances are handled along with the OS theme.
        MainActor.assumeIsolated {
            systemAppearanceRawValue(for: NSApplication.shared.effectiveAppearance)
        }
    }

    static func systemAppearanceRawValue(for appearance: NSAppearance) -> UInt32 {
        appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua ? 1 : 0
    }

    private static func workingDirectory(for config: OpaquePointer?) throws -> String? {
        guard let config else {
            return nil
        }

        var bytes = TermyFfiBytes()
        try TermyFfiBridge.requireOK(
            "termy_config_working_directory",
            termy_config_working_directory(config, &bytes)
        )
        defer {
            if bytes.ptr != nil {
                _ = termy_buffer_free(bytes)
            }
        }

        guard bytes.ptr != nil, bytes.len > 0 else {
            return nil
        }
        let value = TermyFfiBridge.string(from: bytes, trimmingWhitespaceAndNewlines: true) ?? ""
        return value.isEmpty ? nil : value
    }

    private static func normalizedWorkingDirectory(_ value: String?) -> String? {
        guard let value else {
            return nil
        }

        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func event(from event: TermyFfiEvent) -> TerminalRuntimeEvent? {
        guard let eventKind = TerminalRuntimeEventKind(rawValue: event.kind) else {
            return nil
        }

        switch eventKind {
        case .wakeup:
            return .wakeup
        case .title:
            return .title(TermyFfiBridge.string(from: event.payload) ?? "")
        case .resetTitle:
            return .resetTitle
        case .bell:
            return .bell
        case .exit:
            return .exit
        case .clipboardStore:
            return .clipboardStore(TermyFfiBridge.string(from: event.payload) ?? "")
        case .shellPromptStart:
            return .shellPromptStart
        case .shellCommandStart:
            return .shellCommandStart
        case .shellCommandExecuting:
            return .shellCommandExecuting
        case .shellCommandFinished:
            return .shellCommandFinished(event.exit_code >= 0 ? event.exit_code : nil)
        case .progress:
            return .progress(TerminalProgress(
                state: event.progress_state,
                value: event.progress_value
            ))
        case .workingDirectory:
            return .workingDirectory(TermyFfiBridge.string(from: event.payload) ?? "")
        }
    }

    private static func cell(from ffiCell: TermyFfiCell, col: Int, row: Int) -> TerminalCell {
        TerminalCell(
            col: col,
            row: row,
            codepoint: ffiCell.codepoint,
            foreground: TerminalRGBA(ffiCell.fg),
            background: TerminalRGBA(ffiCell.bg),
            usesTerminalDefaultBackground: ffiCell.uses_terminal_default_bg,
            renderText: ffiCell.render_text,
            bold: ffiCell.bold,
            wideCharacterSpacer: ffiCell.wide_character_spacer,
            lineWrapped: ffiCell.line_wrapped
        )
    }

    private static func cursor(from ffiCursor: TermyFfiCursor) -> TerminalCursor? {
        guard ffiCursor.visible else {
            return nil
        }
        return TerminalCursor(
            col: Int(ffiCursor.col),
            row: Int(ffiCursor.row),
            style: TerminalCursorStyle(ffiRawValue: ffiCursor.style)
        )
    }

}
