const support = @import("support");
const encoded_answer = @embedFile("answer.txt");

pub const answer = support.answer + encoded_answer.len - 3;
