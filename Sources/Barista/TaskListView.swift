import BaristaCore
import SwiftUI

/// One compact row for a command, agent, or watched app that is keeping the Mac awake.
struct TaskRow: View {
    let lease: CoffeeLease
    @ObservedObject var session: AwakeSession

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: icon).foregroundStyle(CoffeeTheme.accent).font(.system(size: 13)).frame(width: 16)
            VStack(alignment: .leading, spacing: 3) {
                Text(lease.title).font(.system(size: 12, weight: .medium)).lineLimit(1)
                Text(lease.message ?? detail).font(.system(size: 10)).foregroundStyle(.secondary).lineLimit(1)
                if let progress = lease.progress {
                    ProgressView(value: progress).controlSize(.mini).accessibilityLabel("Task progress")
                }
            }
            Spacer(minLength: 4)
            Button { session.cancel(id: lease.id) } label: {
                Image(systemName: "xmark.circle.fill").foregroundStyle(.secondary)
            }
            .buttonStyle(.plain).help("Stop keeping awake for this task. The task keeps running.")
            .accessibilityLabel("Stop \(lease.title)")
        }
        .padding(9)
        .background(Color.primary.opacity(0.04), in: RoundedRectangle(cornerRadius: 9))
    }

    private var icon: String {
        switch lease.source {
        case .manual: "cup.and.saucer.fill"
        case .agent: "sparkles"
        case .command: "terminal"
        case .application: "app"
        }
    }
    private var detail: String {
        let kind = switch lease.source {
        case .manual: "Your session"
        case .application: "Until the app quits"
        case .agent: "Agent"
        case .command: "Until the command finishes"
        }
        let display = lease.keepDisplayAwake ? "display on" : "display may sleep"
        return "\(kind) · \(display)"
    }
}
