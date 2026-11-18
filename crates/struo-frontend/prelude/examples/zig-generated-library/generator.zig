const std = @import("std");

pub fn main(init: std.process.Init) !void {
    var args = try init.minimal.args.iterateAllocator(init.gpa);
    defer args.deinit();
    _ = args.next();
    const output = args.next() orelse return error.MissingOutput;
    const input = args.next() orelse return error.MissingInput;
    const source = try std.Io.Dir.cwd().readFileAlloc(init.io, input, init.gpa, .limited(1024));
    defer init.gpa.free(source);
    try std.Io.Dir.cwd().writeFile(init.io, .{
        .sub_path = output,
        .data = source,
    });
}
