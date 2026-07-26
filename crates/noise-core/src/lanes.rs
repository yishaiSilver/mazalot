//! A four-lane integer/float shim, so the noise kernels can hash four lattice
//! corners at a time.
//!
//! Two backends, one API:
//!   • **wasm32 + simd128** — `v128` instructions. This is the one that pays:
//!     the browser demos re-shade every pixel every frame.
//!   • **everything else** — plain `[_; 4]` arrays. The compiler unrolls these
//!     into the same scalar instruction stream the code used before, so the
//!     native generators keep their exact float behaviour.
//!
//! **Both backends must produce bit-identical results**, because `out/` is the
//! regression test and a one-ULP drift moves pixels across the Bayer-dither
//! thresholds. That is easy to hold onto here and easy to lose: every operation
//! below is lane-wise IEEE-754 (or exact integer) with no reassociation and no
//! multiply-add contraction. Do not add a `mul_add`/FMA method — wasm's simd128
//! has no fused multiply-add (that is `relaxed-simd`, which we deliberately do
//! not enable), so an FMA would exist on native only and immediately split the
//! two backends' output.
//!
//! `U32x4::from_i32` takes signed lattice coordinates and reinterprets them,
//! matching the `as u32` cast in the scalar hash.

// ---------------------------------------------------------------------------
// wasm32 + simd128
// ---------------------------------------------------------------------------
#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
mod imp {
    use core::arch::wasm32::*;

    #[derive(Clone, Copy)]
    pub struct U32x4(v128);
    #[derive(Clone, Copy)]
    pub struct F32x4(v128);

    impl U32x4 {
        #[inline(always)]
        pub fn from_i32(a: i32, b: i32, c: i32, d: i32) -> Self {
            U32x4(u32x4(a as u32, b as u32, c as u32, d as u32))
        }
        #[inline(always)]
        pub fn splat(v: u32) -> Self {
            U32x4(u32x4_splat(v))
        }
        /// Lane-wise wrapping multiply by a scalar (`i32x4_mul` keeps the low 32
        /// bits, which is what `wrapping_mul` on `u32` does).
        #[inline(always)]
        pub fn wmul(self, k: u32) -> Self {
            U32x4(i32x4_mul(self.0, u32x4_splat(k)))
        }
        /// Lane-wise wrapping add of a scalar.
        #[inline(always)]
        pub fn wadd(self, k: u32) -> Self {
            U32x4(i32x4_add(self.0, u32x4_splat(k)))
        }
        #[inline(always)]
        pub fn xor(self, o: Self) -> Self {
            U32x4(v128_xor(self.0, o.0))
        }
        /// Logical (zero-filling) shift right — `u32 >> n`, never arithmetic.
        #[inline(always)]
        pub fn shr(self, n: u32) -> Self {
            U32x4(u32x4_shr(self.0, n))
        }
        /// `(lane as f32) / (u32::MAX as f32)` — the scalar hash's final step.
        #[inline(always)]
        pub fn to_unit_f32(self) -> F32x4 {
            F32x4(f32x4_div(f32x4_convert_u32x4(self.0), f32x4_splat(u32::MAX as f32)))
        }
        /// Reinterpret the lanes as `i32` and widen to `f32`, exactly. Lets a
        /// lattice coordinate be built once as integers and reused for both the
        /// hash and the distance, instead of assembling a second float vector.
        #[inline(always)]
        pub fn to_i32_f32(self) -> F32x4 {
            F32x4(f32x4_convert_i32x4(self.0))
        }
    }

    impl F32x4 {
        #[inline(always)]
        pub fn add(self, o: Self) -> Self {
            F32x4(f32x4_add(self.0, o.0))
        }
        #[inline(always)]
        pub fn sub(self, o: Self) -> Self {
            F32x4(f32x4_sub(self.0, o.0))
        }
        #[inline(always)]
        pub fn mul(self, o: Self) -> Self {
            F32x4(f32x4_mul(self.0, o.0))
        }
        #[inline(always)]
        pub fn scale(self, k: f32) -> Self {
            F32x4(f32x4_mul(self.0, f32x4_splat(k)))
        }
        #[inline(always)]
        pub fn lanes(self) -> [f32; 4] {
            [
                f32x4_extract_lane::<0>(self.0),
                f32x4_extract_lane::<1>(self.0),
                f32x4_extract_lane::<2>(self.0),
                f32x4_extract_lane::<3>(self.0),
            ]
        }
        /// Lane-wise minimum. NOT `f32x4_pmin`: that one is
        /// "return `b < a ? b : a`", which propagates the second operand's NaN
        /// and differs from `f32::min`. `f32x4_min` matches.
        #[inline(always)]
        pub fn min(self, o: Self) -> Self {
            F32x4(f32x4_min(self.0, o.0))
        }
        #[inline(always)]
        pub fn splat(v: f32) -> Self {
            F32x4(f32x4_splat(v))
        }
        /// Horizontal minimum across the four lanes.
        #[inline(always)]
        pub fn min_lane(self) -> f32 {
            let l = self.lanes();
            l[0].min(l[1]).min(l[2].min(l[3]))
        }
    }
}

// ---------------------------------------------------------------------------
// Portable fallback (native, and wasm without simd128)
// ---------------------------------------------------------------------------
#[cfg(not(all(target_arch = "wasm32", target_feature = "simd128")))]
mod imp {
    #[derive(Clone, Copy)]
    pub struct U32x4([u32; 4]);
    #[derive(Clone, Copy)]
    pub struct F32x4([f32; 4]);

    impl U32x4 {
        #[inline(always)]
        pub fn from_i32(a: i32, b: i32, c: i32, d: i32) -> Self {
            U32x4([a as u32, b as u32, c as u32, d as u32])
        }
        #[inline(always)]
        pub fn splat(v: u32) -> Self {
            U32x4([v; 4])
        }
        #[inline(always)]
        pub fn wmul(self, k: u32) -> Self {
            let a = self.0;
            U32x4([
                a[0].wrapping_mul(k),
                a[1].wrapping_mul(k),
                a[2].wrapping_mul(k),
                a[3].wrapping_mul(k),
            ])
        }
        #[inline(always)]
        pub fn wadd(self, k: u32) -> Self {
            let a = self.0;
            U32x4([
                a[0].wrapping_add(k),
                a[1].wrapping_add(k),
                a[2].wrapping_add(k),
                a[3].wrapping_add(k),
            ])
        }
        #[inline(always)]
        pub fn xor(self, o: Self) -> Self {
            let (a, b) = (self.0, o.0);
            U32x4([a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2], a[3] ^ b[3]])
        }
        #[inline(always)]
        pub fn shr(self, n: u32) -> Self {
            let a = self.0;
            U32x4([a[0] >> n, a[1] >> n, a[2] >> n, a[3] >> n])
        }
        #[inline(always)]
        pub fn to_unit_f32(self) -> F32x4 {
            let a = self.0;
            const M: f32 = u32::MAX as f32;
            F32x4([a[0] as f32 / M, a[1] as f32 / M, a[2] as f32 / M, a[3] as f32 / M])
        }
        #[inline(always)]
        pub fn to_i32_f32(self) -> F32x4 {
            let a = self.0;
            F32x4([a[0] as i32 as f32, a[1] as i32 as f32, a[2] as i32 as f32, a[3] as i32 as f32])
        }
    }

    impl F32x4 {
        #[inline(always)]
        pub fn add(self, o: Self) -> Self {
            let (a, b) = (self.0, o.0);
            F32x4([a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3]])
        }
        #[inline(always)]
        pub fn sub(self, o: Self) -> Self {
            let (a, b) = (self.0, o.0);
            F32x4([a[0] - b[0], a[1] - b[1], a[2] - b[2], a[3] - b[3]])
        }
        #[inline(always)]
        pub fn mul(self, o: Self) -> Self {
            let (a, b) = (self.0, o.0);
            F32x4([a[0] * b[0], a[1] * b[1], a[2] * b[2], a[3] * b[3]])
        }
        #[inline(always)]
        pub fn scale(self, k: f32) -> Self {
            let a = self.0;
            F32x4([a[0] * k, a[1] * k, a[2] * k, a[3] * k])
        }
        #[inline(always)]
        pub fn lanes(self) -> [f32; 4] {
            self.0
        }
        #[inline(always)]
        pub fn min(self, o: Self) -> Self {
            let (a, b) = (self.0, o.0);
            F32x4([a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2]), a[3].min(b[3])])
        }
        #[inline(always)]
        pub fn splat(v: f32) -> Self {
            F32x4([v; 4])
        }
        #[inline(always)]
        pub fn min_lane(self) -> f32 {
            let a = self.0;
            a[0].min(a[1]).min(a[2].min(a[3]))
        }
    }
}

pub use imp::{F32x4, U32x4};
