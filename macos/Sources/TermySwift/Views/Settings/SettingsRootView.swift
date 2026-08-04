import AppKit
import SwiftUI

struct SettingsRootView: View {
    @StateObject private var store = SettingsStore()
    @State private var selection: String?
    @State private var searchText = ""
    @State private var debouncedQuery = ""
    @State private var cachedSections: [SettingsSectionModel] = []
    @State private var debounceTask: Task<Void, Never>?

    var body: some View {
        NavigationSplitView {
            SettingsSidebarView(sections: cachedSections, selection: $selection)
        } detail: {
            if debouncedQuery.trimmingCharacters(in: .whitespaces).isEmpty {
                SettingsDetailView(section: selectedSection, store: store)
            } else {
                SettingsSearchResultsView(
                    results: SettingsSearch.results(in: cachedSections, query: debouncedQuery),
                    store: store
                )
            }
        }
        .searchable(text: $searchText, prompt: "Search settings")
        .frame(minWidth: 760, minHeight: 520)
        .onAppear {
            store.load()
            rebuildCachedSections()
        }
        .onChange(of: store.schema?.sections.map(\.id) ?? []) { _, _ in
            rebuildCachedSections()
        }
        .onChange(of: searchText) { _, newValue in
            debounceSearch(newValue)
        }
        .alert(
            "Settings Error",
            isPresented: Binding(
                get: { store.errorMessage != nil },
                set: { if !$0 { store.errorMessage = nil } }
            )
        ) {
            Button("OK", role: .cancel) { store.errorMessage = nil }
        } message: {
            Text(store.errorMessage ?? "")
        }
    }

    private var selectedSection: SettingsSectionModel? {
        cachedSections.first { $0.id == selection }
    }

    private func rebuildCachedSections() {
        let sections = store.schema?.nativeSettingsSections.filter(\.hasSupportedSettings) ?? []
        cachedSections = sections
        selectDefaultSectionIfNeeded()
    }

    private func selectDefaultSectionIfNeeded() {
        guard !cachedSections.isEmpty else {
            selection = nil
            return
        }
        if selection == nil || !cachedSections.contains(where: { $0.id == selection }) {
            selection = cachedSections[0].id
        }
    }

    private func debounceSearch(_ query: String) {
        debounceTask?.cancel()
        let trimmed = query.trimmingCharacters(in: .whitespaces)
        if trimmed.isEmpty {
            debouncedQuery = ""
            return
        }
        debounceTask = Task {
            try? await Task.sleep(nanoseconds: 150_000_000)
            guard !Task.isCancelled else { return }
            await MainActor.run {
                debouncedQuery = query
            }
        }
    }
}

extension SettingsSchema {
    var nativeSettingsSections: [SettingsSectionModel] {
        let hiddenKeys = SettingsSectionModel.hiddenNativeTabStripKeys
        var sections = sections
            .map { $0.withoutThemeSettings.filteringHiddenSettings(hiddenKeys) }
        if let themesSection {
            let insertIndex = sections.firstIndex { $0.id == "colors" } ?? sections.count
            sections.insert(themesSection, at: insertIndex)
        }
        return sections
    }

    private var themesSection: SettingsSectionModel? {
        let themeKeys = SettingsSectionModel.themeSettingKeys
        let settings = sections
            .flatMap { $0.groups ?? [] }
            .flatMap(\.settings)
            .filter { themeKeys.contains($0.key) }

        guard !settings.isEmpty else {
            return nil
        }

        return SettingsSectionModel(
            id: SettingsSectionModel.themesSectionID,
            label: "Themes",
            systemImage: "paintpalette.fill",
            groups: [SettingsGroup(label: "Theme", settings: settings)],
            colors: nil,
            keybinds: nil
        )
    }
}

extension SettingsSectionModel {
    static let hiddenNativeTabStripKeys: Set<String> = [
        "tab_close_visibility",
        "tab_width_mode",
        "tab_bar_position",
        "tab_switch_modifier_hints",
        "auto_hide_tabbar",
        "show_termy_in_titlebar",
        "onboarding_complete",
        "sidebar_enabled",
        "sidebar_width",
        "native_tab_placement",
    ]
}

extension SettingsSectionModel {
    static let themesSectionID = "themes"
    static let themeSettingKeys: Set<String> = [
        "theme",
        "theme_mode",
        "theme_light",
        "theme_dark",
    ]

    func filteringHiddenSettings(_ hiddenKeys: Set<String>) -> SettingsSectionModel {
        guard let groups else {
            return self
        }
        let filteredGroups = groups.compactMap { group -> SettingsGroup? in
            let settings = group.settings.filter { !hiddenKeys.contains($0.key) }
            return settings.isEmpty ? nil : SettingsGroup(label: group.label, settings: settings)
        }
        return SettingsSectionModel(
            id: id,
            label: label,
            systemImage: systemImage,
            groups: filteredGroups,
            colors: colors,
            keybinds: keybinds
        )
    }

    var hasSupportedSettings: Bool {
        !(groups?.flatMap(\.settings).isEmpty ?? true)
            || !(colors?.isEmpty ?? true)
            || keybinds != nil
    }

    var withoutThemeSettings: SettingsSectionModel {
        guard id == "appearance", let groups else {
            return self
        }

        let nextGroups = groups.compactMap { group -> SettingsGroup? in
            let settings = group.settings.filter { !Self.themeSettingKeys.contains($0.key) }
            return settings.isEmpty ? nil : SettingsGroup(label: group.label, settings: settings)
        }
        return SettingsSectionModel(
            id: id,
            label: label,
            systemImage: systemImage,
            groups: nextGroups,
            colors: colors,
            keybinds: keybinds
        )
    }
}
