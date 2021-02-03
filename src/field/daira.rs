use std::cmp::Ordering::{Less, Greater};
use unroll::unroll_for_loops;
use crate::{add_no_overflow, sub, cmp, nonzero_multiplicative_inverse};

// FIXME: These functions are copypasta from monty.rs
#[inline(always)]
fn mul_add_cy_in(a: u64, b: u64, c: u64, cy_in: u64) -> (u64, u64) {
    let t = (a as u128) * (b as u128) + (c as u128) + (cy_in as u128);
    ((t >> 64) as u64, t as u64)
}

#[inline]
#[unroll_for_loops]
fn mul2(x: [u64; 8]) -> [u64; 8] {
    debug_assert_eq!(x[8-1] >> 63, 0, "Most significant bit should be clear");

    let mut result = [0; 8];
    result[0] = x[0] << 1;
    for i in 1..8 {
        result[i] = x[i] << 1 | x[i - 1] >> 63;
    }
    result
}

#[inline]
#[unroll_for_loops]
fn sqr_4(a: [u64; 4]) -> [u64; 8] {
    let mut res = [0u64; 8];

    // Calculate the off-diagonal part of the square
    // TODO: Note that res is all zeros on the first itertion, so no
    // need to add it
    for i in 0 .. 4 {
        let mut hi_in = 0u64;
        for j in i+1 .. 4 {
            let (hi_out, lo) = mul_add_cy_in(a[j], a[i], res[i + j], hi_in);
            res[i + j] = lo;
            hi_in = hi_out;
        }
        res[i + 4] = hi_in;
    }
    res = mul2(res); // NB: Top and bottom words are zero

    // Calculate and add in the diagonal part
    let mut hi_in = 0u64;
    for i in 0 .. 4 {
        let (hi_out, lo) = mul_add_cy_in(a[i], a[i], res[2*i], hi_in);
        res[2*i] = lo;
        let (t, cy) = res[2*i + 1].overflowing_add(hi_out);
        res[2*i + 1] = t;
        hi_in = cy as u64;
    }
    debug_assert_eq!(hi_in, 0, "Unexpected carry detected");
    res
}

/*
#[inline]
#[unroll_for_loops]
fn mul_4_4(a: [u64; 4], b: [u64; 4]) -> [u64; 8] {
    // Grade school multiplication. To avoid carrying at each of
    // O(n^2) steps, we first add each intermediate product to a
    // 128-bit accumulator, then propagate carries at the end.
    let mut acc128 = [0u128; 4 + 4];

    for i in 0..4 {
        for j in 0..4 {
            let a_i_b_j = a[i] as u128 * b[j] as u128;
            // Add the less significant chunk to the less significant
            // accumulator.
            acc128[i + j] += a_i_b_j as u64 as u128;
            // Add the more significant chunk to the more significant
            // accumulator.
            acc128[i + j + 1] += a_i_b_j >> 64;
        }
    }

    let mut acc = [0u64; 8];
    acc[0] = acc128[0] as u64;
    let mut carry = false;
    for i in 1..8 {
        let last_chunk_big = (acc128[i - 1] >> 64) as u64;
        let curr_chunk_small = acc128[i] as u64;
        // Note that last_chunk_big won't get anywhere near 2^64,
        // since it's essentially a carry from some additions in the
        // previous phase, so we can add the carry bit to it without
        // fear of overflow.
        let result = curr_chunk_small.overflowing_add(
            last_chunk_big + carry as u64);
        acc[i] += result.0;
        carry = result.1;
    }
    debug_assert!(!carry);
    acc
}
*/


#[inline(always)]
fn mul_4_1_step1(a: [u64; 4], b: u64, r: &mut [u64; 4]) -> u64 {
    let mut c: u64;
    unsafe {
        asm!(
            "mov rdx, {b}",             // rdx = b
            "mulx {r0}, {c}, {a0}",     // r0:c = rdx * a[0]
            "",
            "mulx {r1}, {lo}, {a1}",    // r1:lo = rdx * a[1]
            "add {r0}, {lo}",           // CF:r0 = r0 + lo
            "",
            "mulx {r2}, {lo}, {a2}",    // r2:lo = rdx * a[2]
            "adc {r1}, {lo}",           // CF:r1 = r1 + lo + CF
            "",
            "mulx {r3}, {lo}, {a3}",    // r3:lo = rdx * a[3]
            "adc {r2}, {lo}",           // CF:r2 = r2 + lo + CF
            "",
            "adc {r3}, 0",              // CF:r3 = r3 + CF
            "",
            a0 = in(reg) a[0],
            a1 = in(reg) a[1],
            a2 = in(reg) a[2],
            a3 = in(reg) a[3],
            b = in(reg) b,
            lo = out(reg) _,
            c = out(reg) c,
            r0 = out(reg) r[0],
            r1 = out(reg) r[1],
            r2 = out(reg) r[2],
            r3 = out(reg) r[3],
            out("rdx") _,  // TODO: load b directly into rdx
            options(pure, nomem, nostack),
        );
    }
    c
}


#[inline(always)]
fn mul_4_1_step2(a: [u64; 4], b: u64, r: &mut [u64; 4]) -> u64 {
    let mut c: u64;
    unsafe {
        asm!(
            "xor rax, rax", // clear both carry chains
            "mov rdx, {b}",
            "",
            "mulx {hi}, {lo}, {a0}",
            "adox {r0}, {lo}",
            "adcx {r1}, {hi}",
            "mov {c}, {r0}",
            "",
            "mulx {hi}, {r0}, {a1}",
            "adox {r0}, {r1}",
            "adcx {r2}, {hi}",
            "",
            "mulx {hi}, {r1}, {a2}",
            "adox {r1}, {r2}",
            "adcx {r3}, {hi}",
            "",
            "mulx {hi}, {r2}, {a3}",
            "adox {r2}, {r3}",
            "adcx {r3}, {hi}",
            "",
            a0 = in(reg) a[0],
            a1 = in(reg) a[1],
            a2 = in(reg) a[2],
            a3 = in(reg) a[3],
            b = in(reg) b,
            hi = out(reg) _,
            lo = out(reg) _,
            c = out(reg) c,
            r0 = inout(reg) r[0],
            r1 = inout(reg) r[1],
            r2 = inout(reg) r[2],
            r3 = inout(reg) r[3],
            out("rdx") _, // TODO: load b directly into rdx
            options(pure, nomem, nostack),
        );
    }
    c
}

#[inline]
fn mul_4_4(a: [u64; 4], b: [u64; 4]) -> [u64; 8] {
    let mut ab = [0u64; 8];
    let mut r = [0u64; 4];
    ab[0] = mul_4_1_step1(a, b[0], &mut r);
    ab[1] = mul_4_1_step2(a, b[1], &mut r);
    ab[2] = mul_4_1_step2(a, b[2], &mut r);
    ab[3] = mul_4_1_step2(a, b[3], &mut r);
    ab[4] = r[0];
    ab[5] = r[1];
    ab[6] = r[2];
    ab[7] = r[3];
    ab
}


#[inline(always)]
fn mul_2_1_step1(a: [u64; 2], b: u64, r: &mut [u64; 2]) -> u64 {
    let mut c: u64;
    unsafe {
        asm!(
            "mov rdx, {b}",             // rdx = b
            "mulx {r0}, {c}, {a0}",     // r0:c = rdx * a[0]
            "",
            "mulx {r1}, {lo}, {a1}",    // r1:lo = rdx * a[1]
            "add {r0}, {lo}",           // CF:r0 = r0 + lo
            "",
            "adc {r1}, 0",              // CF:r1 = r1 + CF
            "",
            a0 = in(reg) a[0],
            a1 = in(reg) a[1],
            b = in(reg) b,
            lo = out(reg) _,
            c = out(reg) c,
            r0 = out(reg) r[0],
            r1 = out(reg) r[1],
            out("rdx") _,  // TODO: load b directly into rdx
            options(pure, nomem, nostack),
        );
    }
    c
}


#[inline(always)]
fn mul_2_1_step2(a: [u64; 2], b: u64, r: &mut [u64; 2]) -> u64 {
    let mut c: u64;
    unsafe {
        asm!(
            "xor rax, rax", // clear both carry chains
            "mov rdx, {b}",
            "",
            "mulx {hi}, {lo}, {a0}",
            "adox {r0}, {lo}",
            "adcx {r1}, {hi}",
            "mov {c}, {r0}",
            "",
            "mulx {hi}, {r0}, {a1}",
            "adox {r0}, {r1}",
            "adcx {r1}, {hi}",
            "",
            a0 = in(reg) a[0],
            a1 = in(reg) a[1],
            b = in(reg) b,
            hi = out(reg) _,
            lo = out(reg) _,
            c = out(reg) c,
            r0 = inout(reg) r[0],
            r1 = inout(reg) r[1],
            out("rdx") _, // TODO: load b directly into rdx
            options(pure, nomem, nostack),
        );
    }
    c
}

#[inline]
fn mul_2_2(a: [u64; 2], b: [u64; 2]) -> [u64; 4] {
    let mut ab = [0u64; 4];
    let mut r = [0u64; 2];
    ab[0] = mul_2_1_step1(a, b[0], &mut r);
    ab[1] = mul_2_1_step2(a, b[1], &mut r);
    ab[2] = r[0];
    ab[3] = r[1];
    ab
}


#[inline]
fn mul_4_2(a: [u64; 4], b: [u64; 2]) -> [u64; 6] {
    let mut ab = [0u64; 6];
    let mut r = [0u64; 4];
    ab[0] = mul_4_1_step1(a, b[0], &mut r);
    ab[1] = mul_4_1_step2(a, b[1], &mut r);
    ab[2] = r[0];
    ab[3] = r[1];
    ab[4] = r[2];
    ab[5] = r[3];
    ab
}

#[inline]
fn add_6_4(a: [u64; 6], b: [u64; 4]) -> [u64; 6] {
    // TODO: This is slightly wasteful, since we know the two high
    // words are zero.
    let c = [b[0], b[1], b[2], b[3], 0, 0];
    add_no_overflow(a, c)
}

/// Given x = sum_{i=0}^7 xi (2^64)^i, with x < 2^512, return
/// y1,y2,y3,y4,z1,z2,z3,z4 such that x = y + z * 2^254 + w * (2^254)^2
/// where y = sum_{i=0}^3 yi (2^64)^i and z = sum_{i=0}^3 zi (2^64)^i
/// are both < 2^254, and w < 16
#[inline]
fn rebase_8(x: [u64; 8]) -> ([u64; 4], [u64; 4], u64) {
    const MASK: u64 = (1u64 << 62) - 1u64; // 2^62-1

    // bottom half is the same, except the top two bits are dropped
    let y = [x[0], x[1], x[2], x[3] & MASK];

    // shift the top half words up by two bits
    let z = [((x[4] << 2) | (x[3] >> 62)),
             ((x[5] << 2) | (x[4] >> 62)),
             ((x[6] << 2) | (x[5] >> 62)),
             ((x[7] << 2) | (x[6] >> 62)) & MASK];

    // save the very top two bits in w
    let w = x[7] >> 60;

    (y, z, w)
}

#[inline]
fn rebase_6(x: [u64; 6]) -> ([u64; 4], [u64; 2]) {
    const MASK: u64 = (1u64 << 62) - 1u64; // 2^62-1

    debug_assert_eq!(x[5] >> 62, 0, "highest word of x is too big");

    // bottom half is the same, except the top two bits are dropped
    let y = [x[0], x[1], x[2], x[3] & MASK];

    // shift the top words up by two bits
    let z = [((x[4] << 2) | (x[3] >> 62)),
             ((x[5] << 2) | (x[4] >> 62))];

    (y, z)
}

/// This modular arithmetic representation is based on Daira's
/// adaptation of the tricks for moduli of the form 2^n + C.
/// Source: https://hackmd.io/drzN-z-_So28zDLhK2tegw
pub trait DairaRepr {
    /// Order of the field (i.e. modulus for operations); equals 2^n +
    /// C for some n and C.
    const ORDER: [u64; 4];

    const ZERO: [u64; 4] = [0u64; 4];
    const ONE: [u64; 4] = [1u64, 0u64, 0u64, 0u64];

    /// The C in 2^n + C.
    const C: [u64; 2];

    /// The value of x*C^2 for x in 1..16
    const C_SQR_TBL: [[u64; 4]; 15];

    const K_M: [u64; 6];

    // TODO: This is copypasta from monty.rs
    // TODO: Daira's representation actually allows for some redundancy,
    // so reducing is not always necessary; same in sub
    fn daira_add(lhs: [u64; 4], rhs: [u64; 4]) -> [u64; 4] {
        let sum = add_no_overflow(lhs, rhs);
        if cmp(sum, Self::ORDER) == Less {
            sum
        } else {
            sub(sum, Self::ORDER)
        }
    }

    // TODO: This is copypasta from monty.rs
    fn daira_sub(lhs: [u64; 4], rhs: [u64; 4]) -> [u64; 4] {
        if cmp(lhs, rhs) == Less {
            // Underflow occurs, so we compute the difference as `self + (-rhs)`.
            add_no_overflow(lhs, Self::daira_neg(rhs))
        } else {
            // No underflow, so it's faster to subtract directly.
            sub(lhs, rhs)
        }
    }

    // TODO: This is copypasta from monty.rs
    fn daira_neg(limbs: [u64; 4]) -> [u64; 4] {
        if limbs == Self::ZERO {
            Self::ZERO
        } else {
            sub(Self::ORDER, limbs)
        }
    }

    /// Given an 8-word number (usually the result of multiplying or
    /// squaring), reduce it modulo 2^n + C.
    ///
    /// The implementation is a direct translation of the formulae at
    /// the end of https://hackmd.io/drzN-z-_So28zDLhK2tegw
    #[inline]
    fn _reduce(x: [u64; 8]) -> [u64; 4] {
        // x = (x0, x1, x2)
        let (x0, x1, x2) = rebase_8(x);
        // s = C * x1
        let s = mul_4_2(x1, Self::C);
        // t = C^2 * x2 + x0
        let t = if x2 == 0 {
            x0
        } else {
            add_no_overflow(Self::C_SQR_TBL[(x2 - 1) as usize], x0)
        };

        // xp = kM - s + t
        let xp = add_6_4(sub(Self::K_M, s), t);

        // xp = (xp0, xp1)
        let (xp0, xp1) = rebase_6(xp);
        // u = C * xp1
        let u = mul_2_2(Self::C, xp1);

        // return M - u + xp0
        let res = add_no_overflow(sub(Self::ORDER, u), xp0);
        // max_expected = 2^254 + M - 1 = 2^255 + c - 1
        let max_expected = [
            Self::ORDER[0] - 1, Self::ORDER[1],
            Self::ORDER[2], Self::ORDER[3] << 1
        ];
        debug_assert!(cmp(res, max_expected) != Greater,
                      "Semi-reduced value exceeds maximum expected");
        // NB: This is not necessary in general; a different interface
        // could accommodate semi-reduced results.  Some performance
        // testing suggested that the gain (as a proportion of
        // mul/sqr) were too small to notice though. Must be included
        // at the moment to make tests of calling code pass (which
        // require values to always be reduced).
        Self::daira_to_canonical(res)
    }

    #[inline]
    fn daira_multiply(a: [u64; 4], b: [u64; 4]) -> [u64; 4] {
        Self::_reduce(mul_4_4(a, b))
    }

    #[inline]
    fn daira_square(a: [u64; 4]) -> [u64; 4] {
        Self::_reduce(sqr_4(a))
    }

    fn daira_inverse(a: [u64; 4]) -> [u64; 4] {
        nonzero_multiplicative_inverse(a, Self::ORDER)
    }

    fn daira_to_canonical(a: [u64; 4]) -> [u64; 4] {
        if cmp(a, Self::ORDER) == Less {
            a
        } else {
            let b = sub(a, Self::ORDER);
            debug_assert!(cmp(b, Self::ORDER) == Less,
                          "Expected at most one reduction loop");
            b
        }
    }
}
