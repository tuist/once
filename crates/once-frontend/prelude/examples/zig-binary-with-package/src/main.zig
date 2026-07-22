const math = @import("math");
const std = @import("std");

pub fn main() void {
    std.debug.print("{d}\n", .{math.answer});
}
