import SwiftUI

struct WorkspacesView: View {
    @EnvironmentObject private var app: AppModel
    @State private var showSearch = false

    var body: some View {
        List {
            if app.filteredWorkspaces().isEmpty {
                ContentUnavailableView(
                    "No workspaces",
                    systemImage: "folder",
                    description: Text("Tap + to add a workspace folder")
                )
            } else {
                ForEach(app.filteredWorkspaces()) { row in
                    Button {
                        app.openWorkspace(row)
                    } label: {
                        WorkspaceRowView(
                            row: row,
                            count: app.connection.sessions(for: row.path, query: "", showArchived: app.showArchived).count
                        )
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .listStyle(.insetGrouped)
        .navigationTitle("Workspaces")
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                Button {
                    withAnimation(.easeInOut(duration: 0.2)) { showSearch.toggle() }
                } label: {
                    Image(systemName: "magnifyingglass")
                }
                Button {
                    app.showAddWorkspace = true
                } label: {
                    Image(systemName: "plus")
                }
            }
        }
        .safeAreaInset(edge: .top, spacing: 0) {
            if showSearch {
                HStack {
                    Image(systemName: "magnifyingglass").foregroundStyle(.secondary)
                    TextField("Search", text: $app.searchQuery)
                        .textInputAutocapitalization(.never)
                }
                .padding(10)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 12))
                .padding(.horizontal)
                .padding(.bottom, 8)
                .transition(.move(edge: .top).combined(with: .opacity))
            }
        }
        .sheet(isPresented: $app.showSessionSheet) {
            if let ws = app.selectedWorkspace {
                SessionSheet(workspace: ws)
                    .presentationDetents([.medium, .large])
                    .presentationDragIndicator(.visible)
                    .presentationCornerRadius(20)
            }
        }
        .sheet(isPresented: $app.showAddWorkspace) {
            NavigationStack {
                Form {
                    TextField("Path, e.g. ~/code/foo", text: $app.newWorkspacePath)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                .navigationTitle("Add workspace")
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Cancel") { app.showAddWorkspace = false }
                    }
                    ToolbarItem(placement: .confirmationAction) {
                        Button("Add") { app.registerWorkspace() }
                    }
                }
            }
            .presentationDetents([.medium])
        }
        .refreshable { app.connection.refresh() }
    }
}

private struct WorkspaceRowView: View {
    let row: WorkspaceRow
    let count: Int

    var body: some View {
        HStack(spacing: 12) {
            Text(row.label)
                .font(.body)
                .foregroundStyle(.primary)
            Spacer()
            if count > 0 {
                Text("\(count)")
                    .font(.subheadline.weight(.medium))
                    .foregroundStyle(.secondary)
                    .padding(.horizontal, 8)
                    .padding(.vertical, 3)
                    .background(.quaternary, in: Capsule())
            }
            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(.tertiary)
        }
        .contentShape(Rectangle())
    }
}
