fn square_pow(base: f32, squarings: u32) -> f32 {
    var result = base;
    for (var i: u32 = 0u; i < squarings; i = i + 1u) {
        result = result * result;
    }
    return result;
}

fn trailing_zeros(v: u32) -> u32 {
    var count: u32 = 0u;
    var x = v;
    while ((x & 1u) == 0u && count < 32u) {
        x = x >> 1u;
        count = count + 1u;
    }
    return count;
}
