import SwiftUI

enum Notes {
    static func title(for count: Int) -> String { count == 1 ? "1 note" : "\(count) notes" }
}

@main
struct OrbitNotesApp: App {
    var body: some Scene {
        WindowGroup {
            NavigationStack {
                List {
                    Label("Plan a little adventure", systemImage: "sun.max")
                    Label("Make something useful", systemImage: "pencil")
                    Label("Share a good idea", systemImage: "paperplane")
                }
                .navigationTitle("Orbit Notes")
            }
        }
    }
}
