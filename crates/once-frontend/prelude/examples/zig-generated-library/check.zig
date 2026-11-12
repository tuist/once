const std = @import("std");
const generated = @import("generated");

test "generated module is importable" {
    try std.testing.expectEqual(@as(u32, 37), generated.answer);
}
