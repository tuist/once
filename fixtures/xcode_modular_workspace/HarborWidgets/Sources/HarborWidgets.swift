import SwiftUI
import WidgetKit

struct StatusEntry: TimelineEntry {
    let date: Date
}

struct StatusProvider: TimelineProvider {
    func placeholder(in context: Context) -> StatusEntry { StatusEntry(date: Date()) }
    func getSnapshot(in context: Context, completion: @escaping (StatusEntry) -> Void) { completion(StatusEntry(date: Date())) }
    func getTimeline(in context: Context, completion: @escaping (Timeline<StatusEntry>) -> Void) {
        completion(Timeline(entries: [StatusEntry(date: Date())], policy: .never))
    }
}

struct StatusWidget: Widget {
    var body: some WidgetConfiguration {
        StaticConfiguration(kind: "StatusWidget", provider: StatusProvider()) { entry in
            Text(entry.date, style: .time)
        }
    }
}

@main
struct HarborWidgetsBundle: WidgetBundle {
    var body: some Widget {
        StatusWidget()
    }
}
