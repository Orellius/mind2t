// Blocks (S2): command + output grouped by the shell's own OSC 133 marks.
//
// The grid's row classes arrive from the C surface (Mind2tHostFrame.row_semantics); a
// block starts at each run of prompt rows and owns everything until the next one. The
// grouping is pure -- the gutter, the click mapping and the copy actions all read from
// the same [Block], so they cannot disagree about where a block begins.

import AppKit
import CMind2tHost

struct Block: Equatable {
    /// Every row of the block: the prompt run and the rows below it.
    let rows: Range<Int>
    /// The leading prompt run -- where the typed command lives (input-marked cells).
    let promptRows: Range<Int>
}

/// Groups row classes into blocks. Rows above the first prompt belong to no block:
/// without integration there are no marks at all, and a gutter over unmarked scrollback
/// would be guessing.
func computeBlocks(_ classes: [UInt8]) -> [Block] {
    let prompt = UInt8(MIND2T_ROW_PROMPT)
    var blocks: [Block] = []
    var index = 0
    while index < classes.count {
        guard classes[index] == prompt else {
            index += 1
            continue
        }
        let promptStart = index
        while index < classes.count && classes[index] == prompt {
            index += 1
        }
        let promptEnd = index
        while index < classes.count && classes[index] != prompt {
            index += 1
        }
        blocks.append(Block(rows: promptStart..<index, promptRows: promptStart..<promptEnd))
    }
    return blocks
}

extension Session {
    /// The typed command of a block: input-marked cells across its prompt rows. A
    /// wrapped command spans several prompt rows; joined without separators because the
    /// wrap was visual, not typed.
    func command(of block: Block) -> String {
        block.promptRows
            .map { rowText(UInt16($0), semantic: UInt8(MIND2T_ROW_INPUT)) }
            .joined()
            .trimmingCharacters(in: .whitespaces)
    }

    /// The output of a block: every non-prompt row's full text, newline-joined, trailing
    /// empty rows dropped.
    func output(of block: Block) -> String {
        var lines = block.rows
            .filter { !block.promptRows.contains($0) }
            .map { rowText(UInt16($0), semantic: UInt8(MIND2T_TEXT_ALL)) }
        while lines.last?.isEmpty == true {
            lines.removeLast()
        }
        return lines.joined(separator: "\n")
    }
}
