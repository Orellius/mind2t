// The split tree and its arithmetic. No AppKit state, no sessions, no views: this file
// answers "given a tree and a rectangle, which pane owns which pixels", and nothing else.
//
// It is separate from the views for the same reason `ChromeLayout` is. The failure here is
// SILENT - a tiling one point out looks fine at a comfortable window size and leaves a
// seam, or an overlap, at a narrow one. That is the defect the sidebar shipped with
// (`x = width - 260` tiles perfectly at 1120 and overlaps at 300), so the discipline it
// earned applies: the second child's extent is a REMAINDER of the first, never a second
// independent computation of the same fraction.
//
// Panes are keyed by an opaque `Int`, so the gate can build a twelve-leaf tree without
// spawning a single pty.

// CoreGraphics for the rect members, Foundation for NSRect's function family. Deliberately
// NOT AppKit: nothing here may reach a view, and the import is the thing that enforces it.
import CoreGraphics
import Foundation

enum SplitAxis: Equatable {
    /// Side by side. `first` is on the LEFT.
    case horizontal
    /// Stacked. `first` is on TOP, which is the larger y in AppKit's bottom-up space.
    case vertical
}

/// A binary tree. Binary rather than n-ary because a fraction between exactly two children
/// stays honest under a divider drag: with n children every drag redistributes n-1
/// fractions and the rounding has nowhere to go.
indirect enum SplitTree: Equatable {
    case leaf(Int)
    case branch(axis: SplitAxis, fraction: CGFloat, first: SplitTree, second: SplitTree)

    /// Every pane id, in visual order: left to right, top to bottom.
    var leaves: [Int] {
        switch self {
        case .leaf(let id): return [id]
        case .branch(_, _, let first, let second): return first.leaves + second.leaves
        }
    }

    /// Replaces the leaf holding `id` with a branch splitting it against `newID`. Returns
    /// nil when `id` is absent, so a caller cannot silently no-op on a stale id.
    func splitting(_ id: Int, with newID: Int, axis: SplitAxis) -> SplitTree? {
        switch self {
        case .leaf(let existing):
            guard existing == id else { return nil }
            return .branch(
                axis: axis, fraction: 0.5, first: .leaf(existing), second: .leaf(newID))
        case .branch(let existingAxis, let fraction, let first, let second):
            if let replaced = first.splitting(id, with: newID, axis: axis) {
                return .branch(
                    axis: existingAxis, fraction: fraction, first: replaced, second: second)
            }
            if let replaced = second.splitting(id, with: newID, axis: axis) {
                return .branch(
                    axis: existingAxis, fraction: fraction, first: first, second: replaced)
            }
            return nil
        }
    }

    /// Removes a pane and COLLAPSES the branch that held it, so the survivor takes the
    /// whole rectangle. A branch left holding one child would keep a divider with nothing
    /// on the far side of it.
    ///
    /// Returns nil when the tree was that leaf alone: the caller decides whether an empty
    /// tree closes the window.
    func removing(_ id: Int) -> SplitTree? {
        switch self {
        case .leaf(let existing):
            return existing == id ? nil : self
        case .branch(let axis, let fraction, let first, let second):
            guard let left = first.removing(id) else { return second }
            guard let right = second.removing(id) else { return left }
            return .branch(axis: axis, fraction: fraction, first: left, second: right)
        }
    }

    /// Sets the fraction of the branch whose divider is `index`, counted in the same order
    /// `SplitLayout.dividers` returns them.
    func adjusting(dividerAt index: Int, to fraction: CGFloat) -> SplitTree {
        var counter = 0
        return SplitTree.adjust(self, index, fraction, &counter)
    }

    private static func adjust(
        _ node: SplitTree, _ target: Int, _ fraction: CGFloat, _ counter: inout Int
    ) -> SplitTree {
        guard case .branch(let axis, let existing, let first, let second) = node else {
            return node
        }
        // Claimed BEFORE recursing, matching `SplitLayout.walk`, which emits a branch's own
        // divider before descending into its second child. Two orders that disagree would
        // resize a different divider than the one under the pointer.
        let mine = counter
        counter += 1
        let newFirst = adjust(first, target, fraction, &counter)
        let newSecond = adjust(second, target, fraction, &counter)
        return .branch(
            axis: axis, fraction: mine == target ? fraction : existing,
            first: newFirst, second: newSecond)
    }
}

struct SplitPane: Equatable {
    let id: Int
    let rect: NSRect
}

/// A resize handle: the band between two children, and the branch it belongs to.
struct SplitDivider: Equatable {
    let index: Int
    let axis: SplitAxis
    let rect: NSRect
    /// The rectangle the branch occupies, so a drag can turn a point back into a fraction.
    let container: NSRect
}

enum SplitLayout {
    /// The gap between panes. Thin on purpose: it is a seam, not a scrollbar.
    static let divider: CGFloat = 6

    /// The smallest a pane may be driven to BY A DRAG. Not a promise about tiny windows: a
    /// rectangle too small to hold two floors cannot be made to hold them, and the
    /// invariant that survives there is exact tiling, not the floor.
    static let minimumWidth: CGFloat = 120
    static let minimumHeight: CGFloat = 60

    /// Which pane owns which pixels.
    ///
    /// Rects are integral. A terminal grid derives cols and rows from its pane's size, so a
    /// fractional width does not blur an edge - it silently drops a column, and the column
    /// it drops is the last one, which is where the cursor usually is.
    static func tile(_ tree: SplitTree, in rect: NSRect) -> [SplitPane] {
        var panes: [SplitPane] = []
        var dividers: [SplitDivider] = []
        var counter = 0
        walk(tree, NSIntegralRect(rect), &panes, &dividers, &counter)
        return panes
    }

    /// The resize handles, indexed the way `adjusting(dividerAt:)` counts them.
    static func dividers(_ tree: SplitTree, in rect: NSRect) -> [SplitDivider] {
        var panes: [SplitPane] = []
        var dividers: [SplitDivider] = []
        var counter = 0
        walk(tree, NSIntegralRect(rect), &panes, &dividers, &counter)
        return dividers
    }

    private static func walk(
        _ node: SplitTree, _ rect: NSRect, _ panes: inout [SplitPane],
        _ dividers: inout [SplitDivider], _ counter: inout Int
    ) {
        guard case .branch(let axis, let fraction, let first, let second) = node else {
            guard case .leaf(let id) = node else { return }
            panes.append(SplitPane(id: id, rect: rect))
            return
        }

        let total = axis == .horizontal ? rect.width : rect.height
        // A rect too thin to hold even the divider is the degenerate case. Clamping to zero
        // keeps both children non-negative and keeps the sum exact, which is the invariant
        // that has to survive a window dragged down to nothing.
        let band = min(divider, max(0, total))
        let available = max(0, total - band)
        var firstExtent = (available * min(max(0, fraction), 1)).rounded()
        firstExtent = min(max(0, firstExtent), available)
        // THE REMAINDER RULE. Computed as `available * (1 - fraction)` this rounds
        // independently and leaves a one-point seam or overlap at odd sizes, invisible
        // until it is not.
        let secondExtent = available - firstExtent

        let firstRect: NSRect
        let dividerRect: NSRect
        let secondRect: NSRect
        switch axis {
        case .horizontal:
            firstRect = NSRect(
                x: rect.minX, y: rect.minY, width: firstExtent, height: rect.height)
            dividerRect = NSRect(
                x: firstRect.maxX, y: rect.minY, width: band, height: rect.height)
            secondRect = NSRect(
                x: dividerRect.maxX, y: rect.minY, width: secondExtent, height: rect.height)
        case .vertical:
            // `first` is the TOP pane, and AppKit's origin is bottom-left, so it takes the
            // HIGH y. Getting this backwards still tiles exactly, so the tiling assertion
            // cannot see it - the gate asserts the order separately.
            firstRect = NSRect(
                x: rect.minX, y: rect.maxY - firstExtent,
                width: rect.width, height: firstExtent)
            dividerRect = NSRect(
                x: rect.minX, y: firstRect.minY - band, width: rect.width, height: band)
            secondRect = NSRect(
                x: rect.minX, y: rect.minY, width: rect.width, height: secondExtent)
        }

        // The branch's own divider is claimed BEFORE descending, so the index a drag
        // reports matches what `adjusting(dividerAt:)` counts.
        let mine = counter
        counter += 1
        dividers.append(
            SplitDivider(index: mine, axis: axis, rect: dividerRect, container: rect))
        walk(first, firstRect, &panes, &dividers, &counter)
        walk(second, secondRect, &panes, &dividers, &counter)
    }

    /// Turns a divider drag into a fraction, with the pane floor applied here rather than
    /// at the call site: the floor is a property of the layout, and a caller that forgot it
    /// would produce a pane the terminal cannot derive a grid from.
    static func fraction(for divider: SplitDivider, at point: NSPoint) -> CGFloat {
        let container = divider.container
        let total = divider.axis == .horizontal ? container.width : container.height
        let available = max(0, total - min(self.divider, max(0, total)))
        guard available > 0 else { return 0.5 }
        let floor = divider.axis == .horizontal ? minimumWidth : minimumHeight
        let raw: CGFloat
        switch divider.axis {
        case .horizontal: raw = point.x - container.minX
        // Dragging DOWN must shrink the top pane, and the top pane is the high y.
        case .vertical: raw = container.maxY - point.y
        }
        // When the container cannot hold two floors, the floor is unsatisfiable and the
        // honest answer is the middle rather than a clamp that silently favours one side.
        guard available >= floor * 2 else { return 0.5 }
        return min(max(floor, raw), available - floor) / available
    }

    /// The grid a pane of this size holds.
    ///
    /// Pulled out of `gridForPane`, which read the ONE shared view's bounds. With a single
    /// pane that is the same answer; with several it hands every pane the focused pane's
    /// grid, and a pty told it is 200 columns wide inside an 80 column pane wraps in the
    /// wrong place with nothing logged anywhere. The defect is invisible until a line is
    /// long enough, which is why it belongs in arithmetic a gate can reach.
    ///
    /// The floor of 2 is not cosmetic: a zero-column pty is a division by zero in the
    /// reflow, and a one-column one cannot render a cursor beside a glyph.
    static func grid(
        for rect: NSRect, cellWidth: Int, cellHeight: Int, scale: CGFloat, padding: CGFloat
    ) -> (cols: UInt16, rows: UInt16) {
        guard cellWidth > 0, cellHeight > 0 else { return (80, 24) }
        let inner = rect.insetBy(dx: padding, dy: padding)
        let cols = Int(max(0, inner.width) * scale) / cellWidth
        let rows = Int(max(0, inner.height) * scale) / cellHeight
        return (UInt16(max(2, cols)), UInt16(max(2, rows)))
    }

    /// The pane a focus move lands on: GEOMETRIC, not tree-shaped.
    ///
    /// A tree walk would move focus to a sibling that is nowhere near the arrow the
    /// operator pressed - correct by the data structure and wrong by the screen.
    ///
    /// `forward` is right for horizontal and DOWN for vertical, matching the arrow keys.
    static func neighbor(
        of id: Int, among panes: [SplitPane], axis: SplitAxis, forward: Bool
    ) -> Int? {
        guard let from = panes.first(where: { $0.id == id })?.rect else { return nil }
        var best: (id: Int, distance: CGFloat, overlap: CGFloat)?
        for pane in panes where pane.id != id {
            let to = pane.rect
            let distance: CGFloat
            let overlap: CGFloat
            switch axis {
            case .horizontal:
                distance = forward ? to.minX - from.maxX : from.minX - to.maxX
                overlap = min(from.maxY, to.maxY) - max(from.minY, to.minY)
            case .vertical:
                distance = forward ? from.minY - to.maxY : to.minY - from.maxY
                overlap = min(from.maxX, to.maxX) - max(from.minX, to.minX)
            }
            // Must be on the correct side, and must actually face the source pane. Without
            // the overlap test, focus jumps diagonally to a pane that shares no edge.
            guard distance >= 0, overlap > 0 else { continue }
            guard let current = best else {
                best = (pane.id, distance, overlap)
                continue
            }
            if distance < current.distance
                || (distance == current.distance && overlap > current.overlap)
            {
                best = (pane.id, distance, overlap)
            }
        }
        return best?.id
    }
}
