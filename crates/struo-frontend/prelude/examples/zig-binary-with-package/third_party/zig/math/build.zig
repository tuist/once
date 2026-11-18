const std = @import("std");

pub fn build(b: *std.Build) void {
    _ = b.addModule("math", .{
        .root_source_file = b.path("source/entry.zig"),
    });
}
