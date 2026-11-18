const std = @import("std");

test "addition" {
    try std.testing.expectEqual(@as(i32, 4), 2 + 2);
}
