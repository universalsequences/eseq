#include <arm_neon.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <math.h>
#include <Accelerate/Accelerate.h>
#include <mach/mach_time.h>

// Enable profiling only when DGEN_PROFILE is defined by build flags

float32x4_t vfmodq_f32(float32x4_t a, float32x4_t b) {
  // a - floor(a / b) * b  (faster and correct for positive ranges)
  float32x4_t q = vdivq_f32(a, b);
  float32x4_t q_floor = vrndmq_f32(q);  // floor
  return vsubq_f32(a, vmulq_f32(b, q_floor));
}

static inline uint32x4_t mask_nz_f32(float32x4_t x) {
    float32x4_t zero = vdupq_n_f32(0.0f);
    // eq0 = (x == 0.0f)
    uint32x4_t eq0  = vceqq_f32(x, zero);
    // non-zero mask = bitwise NOT of eq0
    return vmvnq_u32(eq0);
}

static inline float32x4_t boolmask_to_float(uint32x4_t m) {
    float32x4_t ones  = vdupq_n_f32(1.0f);
    float32x4_t zeros = vdupq_n_f32(0.0f);
    // Select 1.0f where mask bits are 1, else 0.0f
    return vbslq_f32(m, ones, zeros);
}

static inline float32x4_t simd_and_f32(float32x4_t a, float32x4_t b) {
    uint32x4_t a_nz = mask_nz_f32(a);
    uint32x4_t b_nz = mask_nz_f32(b);
    uint32x4_t m    = vandq_u32(a_nz, b_nz);
    return boolmask_to_float(m);
}

static inline float32x4_t simd_or_f32(float32x4_t a, float32x4_t b) {
    uint32x4_t a_nz = mask_nz_f32(a);
    uint32x4_t b_nz = mask_nz_f32(b);
    uint32x4_t m    = vorrq_u32(a_nz, b_nz);
    return boolmask_to_float(m);
}

static inline float32x4_t simd_xor_f32(float32x4_t a, float32x4_t b) {
    uint32x4_t a_nz = mask_nz_f32(a);
    uint32x4_t b_nz = mask_nz_f32(b);
    uint32x4_t m    = veorq_u32(a_nz, b_nz);
    return boolmask_to_float(m);
}

// Replace NaN/Inf with 0 so a single bad node can't poison the whole graph.
static inline float sanitize_out_f32(float v) {
    return isfinite(v) ? v : 0.0f;
}
static inline float32x4_t sanitize_out_f32x4(float32x4_t v) {
    uint32x4_t finite = vcltq_f32(vabsq_f32(v), vdupq_n_f32(INFINITY));
    return vbslq_f32(finite, v, vdupq_n_f32(0.0f));
}

const int VOICE_COUNT = 1;
const int SCRATCH_STRIDE = 4096;
float t13_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t14_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t15_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t16_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t17_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t18_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t19_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t20_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t21_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t22_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t23_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t26_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t30_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t40_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t58_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t59_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t77_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t108_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t109_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t110_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
// Memory size required: 737758 floats

void setParamValue(int cellId, float val) {
  //memory[cellId] = val;
}

void process(float * restrict const *in, float * restrict const *out, int nframes, void * restrict state, void * restrict buffers) {
  int frameCount = nframes;  // Use audiograph frame count parameter
  int i = 0;
  float32x4_t c1 = vdupq_n_f32(220.0f);
  float32x4_t c2 = vdupq_n_f32(2.4f);
  float32x4_t c3 = vdupq_n_f32(0.25f);
  float32x4_t c4 = vdupq_n_f32(0.22f);
  float32x4_t c5 = vdupq_n_f32(0.01f);
  float32x4_t c6 = vdupq_n_f32(36.0f);
  float32x4_t c7 = vdupq_n_f32(0.31f);
  float32x4_t c8 = vdupq_n_f32(0.0f);
  float32x4_t c9 = vdupq_n_f32(48000.0f);
  float32x4_t c10 = vdupq_n_f32(6.4583332e-06f);
  float32x4_t c11 = vdupq_n_f32(1.0f);
  float32x4_t c12 = vdupq_n_f32(6.283185f);
  float32x4_t c13 = vdupq_n_f32(0.19f);
  float32x4_t c14 = vdupq_n_f32(3.9583333e-06f);
  float32x4_t c15 = vdupq_n_f32(0.6f);
  float32x4_t c16 = vdupq_n_f32(2.0f);
  float32x4_t c17 = vdupq_n_f32(0.24f);
  float32x4_t c18 = vdupq_n_f32(0.9994f);
  float32x4_t c19 = vdupq_n_f32(0.0006f);
  float32x4_t c20 = vdupq_n_f32(18.0f);
  float32x4_t c21 = vdupq_n_f32(3.0f);
  float32x4_t c22 = vdupq_n_f32(54.0f);
  float32x4_t c23 = vdupq_n_f32(9.0f);
  float32x4_t c24 = vdupq_n_f32(8.0f);
  float32x4_t c25 = vdupq_n_f32(7.0f);
  float32x4_t c26 = vdupq_n_f32(6.0f);
  float *memory = (float*)state;
  int voiceIndex = 0;
  if (voiceIndex < 0) voiceIndex = 0;
  if (voiceIndex >= VOICE_COUNT) voiceIndex = VOICE_COUNT - 1;
  int _scratchBase = voiceIndex * SCRATCH_STRIDE;
  float *t13 = t13_g + _scratchBase;
  float *t14 = t14_g + _scratchBase;
  float *t15 = t15_g + _scratchBase;
  float *t16 = t16_g + _scratchBase;
  float *t17 = t17_g + _scratchBase;
  float *t18 = t18_g + _scratchBase;
  float *t19 = t19_g + _scratchBase;
  float *t20 = t20_g + _scratchBase;
  float *t21 = t21_g + _scratchBase;
  float *t22 = t22_g + _scratchBase;
  float *t23 = t23_g + _scratchBase;
  float *t26 = t26_g + _scratchBase;
  float *t30 = t30_g + _scratchBase;
  float *t40 = t40_g + _scratchBase;
  float *t58 = t58_g + _scratchBase;
  float *t59 = t59_g + _scratchBase;
  float *t77 = t77_g + _scratchBase;
  float *t108 = t108_g + _scratchBase;
  float *t109 = t109_g + _scratchBase;
  float *t110 = t110_g + _scratchBase;
  /* frameCount available as function parameter */
  for (int i = 0; i < frameCount; i += 4) {
    /* t23 declared globally */
    /* t22 declared globally */
    /* t21 declared globally */
    /* t20 declared globally */
    /* t19 declared globally */
    /* t18 declared globally */
    /* t17 declared globally */
    /* t16 declared globally */
    /* t15 declared globally */
    /* t14 declared globally */
    /* t13 declared globally */
    float32x4_t simd12 = vld1q_f32(in[0] + i);
    float32x4_t simd13 = vld1q_f32(in[1] + i); vst1q_f32(t13 + i, simd13);
    float32x4_t simd14 = vld1q_f32(in[2] + i); vst1q_f32(t14 + i, simd14);
    float32x4_t simd15 = vld1q_f32(in[3] + i); vst1q_f32(t15 + i, simd15);
    float32x4_t simd16 = vld1q_f32(&memory[737724]); vst1q_f32(t16 + i, simd16);
    float32x4_t simd17 = vld1q_f32(&memory[737728]); vst1q_f32(t17 + i, simd17);
    float32x4_t simd18 = vld1q_f32(&memory[737732]); vst1q_f32(t18 + i, simd18);
    float32x4_t simd19 = vld1q_f32(&memory[737736]); vst1q_f32(t19 + i, simd19);
    float32x4_t simd20 = vld1q_f32(&memory[737740]); vst1q_f32(t20 + i, simd20);
    float32x4_t simd21 = vld1q_f32(&memory[737744]); vst1q_f32(t21 + i, simd21);
    float32x4_t simd22 = vld1q_f32(&memory[737748]); vst1q_f32(t22 + i, simd22);
    float32x4_t simd23 = vld1q_f32(&memory[737752]); vst1q_f32(t23 + i, simd23);
    t19[i] = t19[i];
    t18[i] = t18[i];
    t13[i] = t13[i];
    /* t30 declared globally */
    /* t26 declared globally */
    float32x4_t simd24 = vdivq_f32(simd13, vdupq_n_f32(220.0f));
    float32x4_t simd25 = vminq_f32(simd24, c2);
    float32x4_t simd26 = vmaxq_f32(simd25, c3); vst1q_f32(t26 + i, simd26);
    float32x4_t simd27 = vmulq_f32(simd19, simd18);
    float32x4_t simd28 = vmulq_f32(simd27, simd26);
    float32x4_t simd29 = vminq_f32(simd28, c4);
    float32x4_t simd30 = vmaxq_f32(simd29, c5); vst1q_f32(t30 + i, simd30);
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd17 = vld1q_f32(t17 + i); /* extra */
    t17[i] = t17[i];
    float32x4_t simd15 = vld1q_f32(t15 + i); /* extra */
    t15[i] = t15[i];
    float32x4_t simd14 = vld1q_f32(t14 + i); /* extra */
    t14[i] = t14[i];
    for (int t4 = 0; t4 < 36; t4+=1) {
      float t31 = memory[36 + t4];
      float t32 = t31 * t15[i];
      float t33 = t32 * t17[i];
      float t34 = t33 * t14[i];
      int t35 = i;
      int t36 = t35 * 36;
      int t37 = t36 + t4;
      memory[189 + t37] = t34;
    }
    /* t40 declared globally */
    t40[i] = memory[737756];
    float t41 = t40[i] + 6.4583332e-06;
    float t42 = 0.0 > 0.0f ? 0.0 : t41;
    float t43 = t42;
    float t44 = t43;
    float t45 = floorf(t44);
    float t46 = t45;
    float t47 = t42 - t46;
    memory[737756] = t47;
    float t49 = t47 >= 1.0;
    if (t49) {
      float t51 = t47 - 1.0;
      memory[737756] = t51;
    }
    if (0.0) {
      memory[737756] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 4) {
    float32x4_t simd40 = vld1q_f32(t40 + i); /* extra */
    t40[i] = t40[i];
    /* t58 declared globally */
    float32x4_t simd57 = vmulq_f32(simd40, c12);
    float32x4_t simd58 = vsinf(simd57); vst1q_f32(t58 + i, simd58);
  }
  for (int i = 0; i < frameCount; i += 1) {
    /* t59 declared globally */
    t59[i] = memory[737757];
    float t60 = t59[i] + 3.9583333e-06;
    float t61 = 0.0 > 0.0f ? 0.0 : t60;
    float t62 = t61;
    float t63 = t62;
    float t64 = floorf(t63);
    float t65 = t64;
    float t66 = t61 - t65;
    memory[737757] = t66;
    float t68 = t66 >= 1.0;
    if (t68) {
      float t70 = t66 - 1.0;
      memory[737757] = t70;
    }
    if (0.0) {
      memory[737757] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 4) {
    float32x4_t simd59 = vld1q_f32(t59 + i); /* extra */
    t59[i] = t59[i];
    /* t77 declared globally */
    float32x4_t simd76 = vmulq_f32(simd59, c12);
    float32x4_t simd77 = vsinf(simd76); vst1q_f32(t77 + i, simd77);
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd77 = vld1q_f32(t77 + i); /* extra */
    t77[i] = t77[i];
    float32x4_t simd58 = vld1q_f32(t58 + i); /* extra */
    t58[i] = t58[i];
    float32x4_t simd22 = vld1q_f32(t22 + i); /* extra */
    t22[i] = t22[i];
    for (int t5 = 0; t5 < 36; t5+=1) {
      float t78 = memory[72 + t5];
      float t79 = t78 * t58[i];
      float t80 = t79 * t22[i];
      int t81 = i;
      int t82 = t81 * 36;
      int t83 = t82 + t5;
      memory[147645 + t83] = t80;
      float t85 = memory[108 + t5];
      float t86 = t85 * t77[i];
      float t87 = t86 * t22[i];
      int t88 = i;
      int t89 = t88 * 36;
      int t90 = t89 + t5;
      memory[295101 + t90] = t87;
    }
    for (int t6 = 0; t6 < 36; t6+=1) {
      int t93 = i;
      int t94 = t93 * 36;
      int t95 = t94 + t6;
      float t96 = memory[295101 + t95];
      float t97 = t96 * 0.6;
      int t98 = i;
      int t99 = t98 * 36;
      int t100 = t99 + t6;
      float t101 = memory[147645 + t100];
      float t102 = t101 + t97;
      int t103 = i;
      int t104 = t103 * 36;
      int t105 = t104 + t6;
      memory[442557 + t105] = t102;
    }
  }
  /* [1mUOp[0m(op: [38;5;51mreshape[0m([1, 1, 3, 3]), value: empty) */
  /* [1mUOp[0m(op: [38;5;51mexpandView[0m([6, 6, 3, 3]), value: empty) */
  for (int i = 0; i < frameCount; i += 4) {
    float32x4_t simd30 = vld1q_f32(t30 + i); /* extra */
    t30[i] = t30[i];
    float32x4_t simd20 = vld1q_f32(t20 + i); /* extra */
    t20[i] = t20[i];
    /* t110 declared globally */
    /* t109 declared globally */
    /* t108 declared globally */
    float32x4_t simd108 = vsubq_f32(c16, simd20); vst1q_f32(t108 + i, simd108);
    float32x4_t simd109 = vsubq_f32(c11, simd20); vst1q_f32(t109 + i, simd109);
    float32x4_t simd110 = vmulq_f32(simd30, c19); vst1q_f32(t110 + i, simd110);
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd110 = vld1q_f32(t110 + i); /* extra */
    t110[i] = t110[i];
    float32x4_t simd109 = vld1q_f32(t109 + i); /* extra */
    t109[i] = t109[i];
    float32x4_t simd108 = vld1q_f32(t108 + i); /* extra */
    t108[i] = t108[i];
    float32x4_t simd26 = vld1q_f32(t26 + i); /* extra */
    t26[i] = t26[i];
    float32x4_t simd21 = vld1q_f32(t21 + i); /* extra */
    t21[i] = t21[i];
    float32x4_t simd18 = vld1q_f32(t18 + i); /* extra */
    t18[i] = t18[i];
    for (int t111 = 0; t111 < 36; t111++) {
      float t112 = memory[590193 + (isfinite((int) t111) ? (int) t111 : 0)];
      float t113 = memory[590229 + (isfinite((int) t111) ? (int) t111 : 0)];
      memory[590157 + (int)t111] = t113;
      float t115 = memory[0 + (isfinite((int) t111) ? (int) t111 : 0)];
      memory[590121 + (int)t111] = t115;
      int t117 = i;
      int t118 = t117 * 36;
      int t119 = t118 + t111;
      float t120 = memory[189 + t119];
      float t121 = t112 + t120;
      memory[147645 + (int)t111] = t121;
      /* [1mUOp[0m(op: [38;5;51mpad[0m([(1, 1), (1, 1)]), value: empty) */
    }
    for (int t123 = 0; t123 < 324; t123++) {
      /* [1mUOp[0m(op: [38;5;51mreshape[0m([6, 6, 3, 3]), value: empty) */
    }
    for (int t124 = 0; t124 < 108; t124++) {
      /* [1mUOp[0m(op: [38;5;51msumAxisMarker[0m(node=59, axis=3, in=[6, 6, 3, 3], out=[6, 6, 3], inFA=false, outFA=false), value: empty) */
      float t125 = 0.0;
      int t126 = t124 / 18;
      int t127 = t126 * 18;
      int t128 = t124 - t127;
      int t129 = t128 / 3;
      int t130 = t129 * 3;
      int t131 = t128 - t130;
      int t132 = t131;
      int t133 = t132;
      int t134 = t131 - t133;
      int t135 = t126 * 54;
      int t136 = t135;
      int t137 = t129 * 9;
      int t138 = t136 + t137;
      int t139 = t132 * 3;
      int t140 = t138 + t139;
      for (int t141 = 0; t141 < 3; t141++) {
        int t142 = t141;
        int t143 = t140 + t142;
        int t144 = t126 * 8;
        int t145 = t144 + t129;
        int t146 = t132 * 8;
        int t147 = t145 + t146;
        int t148 = t147 + t141;
        int t149 = t148 / 8;
        int t150 = t149 * 8;
        int t151 = t148 - t150;
        int t152 = t149 >= 1;
        int t153 = t149 < 7;
        float t154 = 1.0 * t152;
        float t155 = t154 * t153;
        int t156 = t149 - 1;
        int t157 = t151 >= 1;
        int t158 = t151 < 7;
        float t159 = t155 * t157;
        float t160 = t159 * t158;
        int t161 = t151 - 1;
        int t162 = t156 * 6;
        int t163 = t162 + t161;
        float t164 = 0.0;
        if (t160) {
          float t166 = memory[147645 + t163];
          t164 = t166;
        }
        int t168 = t132 * 3;
        int t169 = t168 + t141;
        float t170 = memory[144 + t169];
        float t171 = t164 * t170;
        float t172 = t125 + t171;
        t125 = t172;
      }
      memory[590013 + (int)t124] = t125;
    }
    for (int t175 = 0; t175 < 36; t175++) {
      /* [1mUOp[0m(op: [38;5;51msumAxisMarker[0m(node=60, axis=2, in=[6, 6, 3], out=[6, 6], inFA=false, outFA=false), value: empty) */
      float t176 = 0.0;
      int t177 = t175 / 6;
      int t178 = t177 * 6;
      int t179 = t175 - t178;
      int t180 = t179;
      int t181 = t180;
      int t182 = t179 - t181;
      int t183 = t177 * 18;
      int t184 = t183;
      int t185 = t180 * 3;
      int t186 = t184 + t185;
      for (int t187 = 0; t187 < 3; t187++) {
        int t188 = t187;
        int t189 = t186 + t188;
        float t190 = memory[590013 + t189];
        float t191 = t176 + t190;
        t176 = t191;
      }
      memory[295101 + (int)t175] = t176;
      float t194 = memory[590121 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t195 = t194 * t26[i];
      float t196 = t195 * t18[i];
      float t197 = fminf(t196, 0.24);
      float t198 = fmaxf(t197, 0.01);
      float t199 = memory[147645 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t200 = t199 * t108[i];
      float t201 = memory[590157 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t202 = t201 * t109[i];
      float t203 = t200 - t202;
      float t204 = memory[295101 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t205 = t204 * t198;
      float t206 = t203 + t205;
      int t207 = i;
      int t208 = t207 * 36;
      int t209 = t208 + t175;
      memory[590265 + t209] = t206;
      float t211 = memory[147645 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t212 = memory[590157 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t213 = t211 - t212;
      float t214 = memory[590121 + (isfinite((int) t175) ? (int) t175 : 0)];
      float t215 = t214 * 0.9994;
      float t216 = t215 + t110[i];
      float t217 = memory[147645 + (isfinite((int) t175) ? (int) t175 : 0)];
      memory[590229 + (int)t175] = t217;
      memory[590193 + (int)t175] = t206;
      float t220 = t213 * t213;
      float t221 = t220 * t21[i];
      float t222 = t216 + t221;
      int t223 = i;
      int t224 = t223 * 36;
      int t225 = t224 + t175;
      float t226 = memory[442557 + t225];
      float t227 = t222 + t226;
      float t228 = fminf(t227, 0.24);
      float t229 = fmaxf(t228, 0.01);
      memory[0 + (int)t175] = t229;
    }
    for (int t11 = 0; t11 < 36; t11+=1) {
      int t231 = i;
      int t232 = t231 * 36;
      int t233 = t232 + t11;
      float t234 = memory[590265 + t233];
      float t235 = memory[153 + t11];
      float t236 = t234 * t235;
      int t237 = i;
      int t238 = t237 * 36;
      int t239 = t238 + t11;
      memory[189 + t239] = t236;
    }
    float32x4_t simd23 = vld1q_f32(t23 + i); /* extra */
    t23[i] = t23[i];
    float32x4_t simd16 = vld1q_f32(t16 + i); /* extra */
    t16[i] = t16[i];
    float t242 = 0.0;
    for (int t243 = 0; t243 < 36; t243++) {
      int t244 = i;
      int t245 = t244 * 36;
      int t246 = t245 + t243;
      float t247 = memory[189 + t246];
      float t248 = t242 + t247;
      t242 = t248;
    }
    float t250 = t242 * t23[i];
    float t251 = tanhf(t250);
    float t252 = t251 * t16[i];
    out[0][i] = sanitize_out_f32(t252);
  }
}