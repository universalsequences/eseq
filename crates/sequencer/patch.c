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
const int SCRATCH_STRIDE = 512;
float t64_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t65_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t66_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t67_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t68_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t69_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t70_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t71_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t72_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t73_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t74_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t75_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t76_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t77_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t78_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t79_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t80_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t85_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t102_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t103_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t104_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t143_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t160_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t161_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t162_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t258_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t272_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t274_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t291_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t308_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t309_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t332_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t333_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t407_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t447_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t473_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t499_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t505_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t538_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t571_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t605_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t616_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t650_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t662_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t664_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t665_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t680_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t681_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t735_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t768_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t1041_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t1080_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
float t1119_g[VOICE_COUNT * SCRATCH_STRIDE] __attribute__((aligned(64))) = {0};
// Memory size required: 6837284 floats

void setParamValue(int cellId, float val) {
  //memory[cellId] = val;
}

void process(float * restrict const *in, float * restrict const *out, int nframes, void * restrict state, void * restrict buffers) {
  int frameCount = nframes;  // Use audiograph frame count parameter
  int i = 0;
  float32x4_t c1 = vdupq_n_f32(0.0f);
  float32x4_t c2 = vdupq_n_f32(1.0f);
  float32x4_t c3 = vdupq_n_f32(1535.0f);
  float32x4_t c4 = vdupq_n_f32(512.0f);
  float32x4_t c5 = vdupq_n_f32(1024.0f);
  float32x4_t c6 = vdupq_n_f32(4.0f);
  float32x4_t c7 = vdupq_n_f32(44100.0f);
  float32x4_t c8 = vdupq_n_f32(6.283185f);
  float32x4_t c9 = vdupq_n_f32(0.5f);
  float32x4_t c10 = vdupq_n_f32(8.0f);
  float32x4_t c11 = vdupq_n_f32(1.5f);
  float32x4_t c12 = vdupq_n_f32(0.25f);
  float32x4_t c13 = vdupq_n_f32(4.3f);
  float32x4_t c14 = vdupq_n_f32(0.7f);
  float32x4_t c15 = vdupq_n_f32(18.0f);
  float32x4_t c16 = vdupq_n_f32(0.45f);
  float32x4_t c17 = vdupq_n_f32(2.0f);
  float32x4_t c18 = vdupq_n_f32(5.65f);
  float32x4_t c19 = vdupq_n_f32(0.35f);
  float32x4_t c20 = vdupq_n_f32(0.82f);
  float32x4_t c21 = vdupq_n_f32(0.65f);
  float32x4_t c22 = vdupq_n_f32(0.0009765625f);
  float *memory = (float*)state;
  int voiceIndex = 0;
  if (voiceIndex < 0) voiceIndex = 0;
  if (voiceIndex >= VOICE_COUNT) voiceIndex = VOICE_COUNT - 1;
  int _scratchBase = voiceIndex * SCRATCH_STRIDE;
  float *t64 = t64_g + _scratchBase;
  float *t65 = t65_g + _scratchBase;
  float *t66 = t66_g + _scratchBase;
  float *t67 = t67_g + _scratchBase;
  float *t68 = t68_g + _scratchBase;
  float *t69 = t69_g + _scratchBase;
  float *t70 = t70_g + _scratchBase;
  float *t71 = t71_g + _scratchBase;
  float *t72 = t72_g + _scratchBase;
  float *t73 = t73_g + _scratchBase;
  float *t74 = t74_g + _scratchBase;
  float *t75 = t75_g + _scratchBase;
  float *t76 = t76_g + _scratchBase;
  float *t77 = t77_g + _scratchBase;
  float *t78 = t78_g + _scratchBase;
  float *t79 = t79_g + _scratchBase;
  float *t80 = t80_g + _scratchBase;
  float *t85 = t85_g + _scratchBase;
  float *t102 = t102_g + _scratchBase;
  float *t103 = t103_g + _scratchBase;
  float *t104 = t104_g + _scratchBase;
  float *t143 = t143_g + _scratchBase;
  float *t160 = t160_g + _scratchBase;
  float *t161 = t161_g + _scratchBase;
  float *t162 = t162_g + _scratchBase;
  float *t258 = t258_g + _scratchBase;
  float *t272 = t272_g + _scratchBase;
  float *t274 = t274_g + _scratchBase;
  float *t291 = t291_g + _scratchBase;
  float *t308 = t308_g + _scratchBase;
  float *t309 = t309_g + _scratchBase;
  float *t332 = t332_g + _scratchBase;
  float *t333 = t333_g + _scratchBase;
  float *t407 = t407_g + _scratchBase;
  float *t447 = t447_g + _scratchBase;
  float *t473 = t473_g + _scratchBase;
  float *t499 = t499_g + _scratchBase;
  float *t505 = t505_g + _scratchBase;
  float *t538 = t538_g + _scratchBase;
  float *t571 = t571_g + _scratchBase;
  float *t605 = t605_g + _scratchBase;
  float *t616 = t616_g + _scratchBase;
  float *t650 = t650_g + _scratchBase;
  float *t662 = t662_g + _scratchBase;
  float *t664 = t664_g + _scratchBase;
  float *t665 = t665_g + _scratchBase;
  float *t680 = t680_g + _scratchBase;
  float *t681 = t681_g + _scratchBase;
  float *t735 = t735_g + _scratchBase;
  float *t768 = t768_g + _scratchBase;
  float *t1041 = t1041_g + _scratchBase;
  float *t1080 = t1080_g + _scratchBase;
  float *t1119 = t1119_g + _scratchBase;
  /* frameCount available as function parameter */
  for (int i = 0; i < frameCount; i += 4) {
    /* t80 declared globally */
    /* t79 declared globally */
    /* t78 declared globally */
    /* t77 declared globally */
    /* t76 declared globally */
    /* t75 declared globally */
    /* t74 declared globally */
    /* t73 declared globally */
    /* t72 declared globally */
    /* t71 declared globally */
    /* t70 declared globally */
    /* t69 declared globally */
    /* t68 declared globally */
    /* t67 declared globally */
    /* t66 declared globally */
    /* t65 declared globally */
    /* t64 declared globally */
    float32x4_t simd64 = vld1q_f32(in[0] + i); vst1q_f32(t64 + i, simd64);
    float32x4_t simd65 = vld1q_f32(in[1] + i); vst1q_f32(t65 + i, simd65);
    float32x4_t simd66 = vdupq_n_f32(memory[6825984 + (int)0.0]); vst1q_f32(t66 + i, simd66);
    float32x4_t simd67 = vdupq_n_f32(memory[6825985 + (int)0.0]); vst1q_f32(t67 + i, simd67);
    float32x4_t simd68 = vdupq_n_f32(memory[6825986 + (int)0.0]); vst1q_f32(t68 + i, simd68);
    float32x4_t simd69 = vdupq_n_f32(memory[6825987 + (int)0.0]); vst1q_f32(t69 + i, simd69);
    float32x4_t simd70 = vdupq_n_f32(memory[6825988 + (int)0.0]); vst1q_f32(t70 + i, simd70);
    float32x4_t simd71 = vdupq_n_f32(memory[6825989 + (int)0.0]); vst1q_f32(t71 + i, simd71);
    float32x4_t simd72 = vdupq_n_f32(memory[6825990 + (int)0.0]); vst1q_f32(t72 + i, simd72);
    float32x4_t simd73 = vdupq_n_f32(memory[6825991 + (int)0.0]); vst1q_f32(t73 + i, simd73);
    float32x4_t simd74 = vdupq_n_f32(memory[6825992 + (int)0.0]); vst1q_f32(t74 + i, simd74);
    float32x4_t simd75 = vdupq_n_f32(memory[6825993 + (int)0.0]); vst1q_f32(t75 + i, simd75);
    float32x4_t simd76 = vdupq_n_f32(memory[6825994 + (int)0.0]); vst1q_f32(t76 + i, simd76);
    float32x4_t simd77 = vdupq_n_f32(memory[6825995 + (int)0.0]); vst1q_f32(t77 + i, simd77);
    float32x4_t simd78 = vdupq_n_f32(memory[6825996 + (int)0.0]); vst1q_f32(t78 + i, simd78);
    float32x4_t simd79 = vdupq_n_f32(memory[6825997 + (int)0.0]); vst1q_f32(t79 + i, simd79);
    float32x4_t simd80 = vdupq_n_f32(memory[6825998 + (int)0.0]); vst1q_f32(t80 + i, simd80);
  }
  for (int simd2 = 0; simd2 < 1024; simd2+=4) {
    float32x4_t simd81 = vld1q_f32(&memory[0 + (int)simd2]);
    float32x4_t simd82 = vsqrtf(simd81);
    vst1q_f32(&memory[4096 + (int)simd2], simd82);
  }
  for (int i = 0; i < frameCount; i += 1) {
    /* t85 declared globally */
    t85[i] = memory[6827534];
    float t86 = t85[i] + 1.0;
    float t87 = 0.0 > 0.0f ? 0.0 : t86;
    float t88 = t87;
    float t89 = (t88 / 1535.0f);
    float t90 = floorf(t89);
    float t91 = t90 * 1535.0;
    float t92 = t87 - t91;
    memory[6827534] = t92;
    float t94 = t92 >= 1535.0;
    if (t94) {
      float t96 = t92 - 1535.0;
      memory[6827534] = t96;
    }
    if (0.0) {
      memory[6827534] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 4) {
    float32x4_t simd85 = vld1q_f32(t85 + i); /* extra */
    t85[i] = t85[i];
    /* t102 declared globally */
    float32x4_t simd102 = vrndmq_f32(simd85); vst1q_f32(t102 + i, simd102);
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
    float32x4_t simd64 = vld1q_f32(t64 + i); /* extra */
    t64[i] = t64[i];
    /* t103 declared globally */
    memory[6825999 + (int)t102[i]] = t64[i];
    /* t104 declared globally */
    t104[i] = memory[6827535];
    float t105 = t104[i] + 1.0;
    float t106 = 0.0 > 0.0f ? 0.0 : t105;
    float t107 = t106;
    float t108 = (t107 / 512.0f);
    float t109 = floorf(t108);
    float t110 = t109 * 512.0;
    float t111 = t106 - t110;
    memory[6827535] = t111;
    float t113 = t111 >= 512.0;
    if (t113) {
      float t115 = t111 - 512.0;
      memory[6827535] = t115;
    }
    if (0.0) {
      memory[6827535] = 0.0;
    }
    /* t143 declared globally */
    t143[i] = memory[6829071];
    float t144 = t143[i] + 1.0;
    float t145 = 0.0 > 0.0f ? 0.0 : t144;
    float t146 = t145;
    float t147 = (t146 / 1535.0f);
    float t148 = floorf(t147);
    float t149 = t148 * 1535.0;
    float t150 = t145 - t149;
    memory[6829071] = t150;
    float t152 = t150 >= 1535.0;
    if (t152) {
      float t154 = t150 - 1535.0;
      memory[6829071] = t154;
    }
    if (0.0) {
      memory[6829071] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 4) {
    float32x4_t simd143 = vld1q_f32(t143 + i); /* extra */
    t143[i] = t143[i];
    /* t160 declared globally */
    float32x4_t simd160 = vrndmq_f32(simd143); vst1q_f32(t160 + i, simd160);
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
    float32x4_t simd65 = vld1q_f32(t65 + i); /* extra */
    t65[i] = t65[i];
    /* t161 declared globally */
    memory[6827536 + (int)t160[i]] = t65[i];
    /* t162 declared globally */
    t162[i] = memory[6829072];
    float t163 = t162[i] + 1.0;
    float t164 = 0.0 > 0.0f ? 0.0 : t163;
    float t165 = t164;
    float t166 = (t165 / 512.0f);
    float t167 = floorf(t166);
    float t168 = t167 * 512.0;
    float t169 = t164 - t168;
    memory[6829072] = t169;
    float t171 = t169 >= 512.0;
    if (t171) {
      float t173 = t169 - 512.0;
      memory[6829072] = t173;
    }
    if (0.0) {
      memory[6829072] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd104 = vld1q_f32(t104 + i); /* extra */
    t104[i] = t104[i];
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd103 = vld1q_f32(t103 + i); /* extra */
    t103[i] = t103[i];
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t121 = 0; t121 < 1024; t121++) {
        /* [1mUOp[0m(op: [38;5;51mreshape[0m([1024]), value: empty) */
      }
      for (int t122 = 0; t122 < 1024; t122++) {
        int t123 = t122;
        int t124 = t123;
        int t125 = t124 / 1024;
        int t126 = t125 * 1024;
        int t127 = t124 - t126;
        float t128 = (int)t102[i];
        int t129 = t128 - 1024;
        int t130 = t129 + 1;
        int t131 = t130 + t127;
        int t132 = t131 + 1535;
        int t133 = t132 % 1535;
        int t134 = t125 * 1535;
        int t135 = t134 + t133;
        float t136 = memory[6825999 + t135];
        float t137 = memory[4096 + (isfinite((int) t122) ? (int) t122 : 0)];
        float t138 = t136 * t137;
        int t139 = i;
        int t140 = t139 * 1024;
        int t141 = t140 + t122;
        memory[5120 + t141] = t138;
      }
    }
    float32x4_t simd162 = vld1q_f32(t162 + i); /* extra */
    t162[i] = t162[i];
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd161 = vld1q_f32(t161 + i); /* extra */
    t161[i] = t161[i];
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      for (int t179 = 0; t179 < 1024; t179++) {
        /* [1mUOp[0m(op: [38;5;51mreshape[0m([1024]), value: empty) */
      }
      for (int t180 = 0; t180 < 1024; t180++) {
        int t181 = t180;
        int t182 = t181;
        int t183 = t182 / 1024;
        int t184 = t183 * 1024;
        int t185 = t182 - t184;
        float t186 = (int)t160[i];
        int t187 = t186 - 1024;
        int t188 = t187 + 1;
        int t189 = t188 + t185;
        int t190 = t189 + 1535;
        int t191 = t190 % 1535;
        int t192 = t183 * 1535;
        int t193 = t192 + t191;
        float t194 = memory[6827536 + t193];
        float t195 = memory[4096 + (isfinite((int) t180) ? (int) t180 : 0)];
        float t196 = t194 * t195;
        int t197 = i;
        int t198 = t197 * 1024;
        int t199 = t198 + t180;
        memory[529408 + t199] = t196;
      }
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t201 = 0; t201 < 1024; t201++) {
        int t202 = i;
        int t203 = t202 * 1024;
        int t204 = t203 + t201;
        float t205 = memory[5120 + t204];
        memory[1053696 + (int)t201] = t205;
        memory[1054720 + (int)t201] = 0.0;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[1053696], .imagp = &memory[1054720] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Forward);
}
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      for (int t210 = 0; t210 < 1024; t210++) {
        int t211 = i;
        int t212 = t211 * 1024;
        int t213 = t212 + t210;
        float t214 = memory[529408 + t213];
        memory[5120 + (int)t210] = t214;
        memory[1055744 + (int)t210] = 0.0;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[5120], .imagp = &memory[1055744] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Forward);
}
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t8 = 0; t8 < 1024; t8+=1) {
        float t219 = memory[1053696 + t8];
        float t220 = memory[1053696 + t8];
        float t221 = t219 * t220;
        float t222 = memory[1054720 + t8];
        float t223 = memory[1054720 + t8];
        float t224 = t222 * t223;
        float t225 = t221 + t224;
        float t226 = sqrtf(t225);
        int t227 = i;
        int t228 = t227 * 1024;
        int t229 = t228 + t8;
        memory[529408 + t229] = t226;
        float t231 = memory[1054720 + t8];
        float t232 = memory[1053696 + t8];
        float t233 = atan2f(t231, t232);
        int t234 = i;
        int t235 = t234 * 1024;
        int t236 = t235 + t8;
        memory[1056768 + t236] = t233;
        float t238 = memory[5120 + t8];
        float t239 = memory[5120 + t8];
        float t240 = t238 * t239;
        float t241 = memory[1055744 + t8];
        float t242 = memory[1055744 + t8];
        float t243 = t241 * t242;
        float t244 = t240 + t243;
        float t245 = sqrtf(t244);
        int t246 = i;
        int t247 = t246 * 1024;
        int t248 = t247 + t8;
        memory[1581056 + t248] = t245;
        float t250 = memory[1055744 + t8];
        float t251 = memory[5120 + t8];
        float t252 = atan2f(t250, t251);
        int t253 = i;
        int t254 = t253 * 1024;
        int t255 = t254 + t8;
        memory[2105344 + t255] = t252;
      }
    }
  }
  float32x4_t simd72 = vld1q_f32(t72 + i); /* extra */
    t72[i] = t72[i];
  /* t258 declared globally */
  float32x4_t simd258 = vsubq_f32(c2, simd72); vst1q_f32(t258 + i, simd258);
  t258[i] = t258[0];
  /* skip scalar load */
  for (int simd12 = 0; simd12 < 1024; simd12+=4) {
    float32x4_t simd259 = vld1q_f32(&memory[1024 + (int)simd12]);
    float32x4_t simd260 = vmulq_f32(simd259, simd258);
    float32x4_t simd261 = vld1q_f32(&memory[2048 + (int)simd12]);
    float32x4_t simd262 = vmulq_f32(simd261, simd72);
    float32x4_t simd263 = vaddq_f32(simd260, simd262);
    vst1q_f32(&memory[2629632 + (int)simd12], simd263);
  }
  float32x4_t simd67 = vld1q_f32(t67 + i); /* extra */
    t67[i] = t67[i];
  for (int simd13 = 0; simd13 < 1024; simd13+=4) {
    float32x4_t simd266 = vld1q_f32(&memory[2629632 + (int)simd13]);
    float32x4_t simd267 = simd266;
    vst1q_f32(&memory[2630656 + (int)simd13], simd267);
  }
  float32x4_t simd73 = vld1q_f32(t73 + i); /* extra */
    t73[i] = t73[i];
  float32x4_t simd70 = vld1q_f32(t70 + i); /* extra */
    t70[i] = t70[i];
  /* t272 declared globally */
  float32x4_t simd270 = vmulq_f32(simd73, c6);
  float32x4_t simd271 = vaddq_f32(simd270, c2);
  float32x4_t simd272 = vmulq_f32(simd70, simd271); vst1q_f32(t272 + i, simd272);
  for (int i = 0; i < frameCount; i += 1) {
    t272[i] = t272[0];
    /* t274 declared globally */
    float t273 = (t272[0] / 44100.0f);
    t274[i] = memory[6829073];
    float t275 = t274[i] + t273;
    float t276 = 0.0 > 0.0f ? 0.0 : t275;
    float t277 = t276;
    float t278 = t277;
    float t279 = floorf(t278);
    float t280 = t279;
    float t281 = t276 - t280;
    memory[6829073] = t281;
    float t283 = t281 >= 1.0;
    if (t283) {
      float t285 = t281 - 1.0;
      memory[6829073] = t285;
    }
    if (0.0) {
      memory[6829073] = 0.0;
    }
    /* t291 declared globally */
    t291[i] = memory[6829074];
    float t292 = t291[i] + 1.0;
    float t293 = 0.0 > 0.0f ? 0.0 : t292;
    float t294 = t293;
    float t295 = (t294 / 512.0f);
    float t296 = floorf(t295);
    float t297 = t296 * 512.0;
    float t298 = t293 - t297;
    memory[6829074] = t298;
    float t300 = t298 >= 512.0;
    if (t300) {
      float t302 = t298 - 512.0;
      memory[6829074] = t302;
    }
    if (0.0) {
      memory[6829074] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      /* t308 declared globally */
      t308[i] = t291[i] == 0.0;
    }
    float32x4_t simd308 = vld1q_f32(t308 + i); /* extra */
    t308[i] = t308[i];
    float32x4_t simd274 = vld1q_f32(t274 + i); /* extra */
    t274[i] = t274[i];
    /* t309 declared globally */
    t309[i] = memory[6829075];
    float t310 = t308[i] > 0.0;
    if (t310) {
      memory[6829075] = t274[i];
      t309[i] = t274[i];
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd309 = vld1q_f32(t309 + i); /* extra */
    t309[i] = t309[i];
      /* skip scalar load */
      /* skip scalar load */
      float32x4_t simd66 = vld1q_f32(t66 + i); /* extra */
    t66[i] = t66[i];
      for (int t14 = 0; t14 < 1024; t14+=1) {
        float t314 = memory[2630656 + t14];
        float t315 = t314 * t66[i];
        float t316 = t315 + t67[i];
        float t317 = t316 + t309[i];
        int t318 = i;
        int t319 = t318 * 1024;
        int t320 = t319 + t14;
        memory[2631680 + t320] = t317;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd309 = vld1q_f32(t309 + i); /* extra */
    t309[i] = t309[i];
      /* skip scalar load */
      float32x4_t simd75 = vld1q_f32(t75 + i); /* extra */
    t75[i] = t75[i];
      float32x4_t simd71 = vld1q_f32(t71 + i); /* extra */
    t71[i] = t71[i];
      /* t333 declared globally */
      /* t332 declared globally */
      float t323 = t309[i] * 6.283185;
      float t324 = cosf(t323);
      float t325 = t324 + 1.0;
      float t326 = t325 * 0.5;
      float t327 = t71[i] * 8.0;
      float t328 = t327 + 1.0;
      float t329 = powf(t326, t328);
      float t330 = t71[i] * 1.5;
      float t331 = t330 * t329;
      t332[i] = t331 + 1.0;
      t333[i] = t75[i] * 0.25;
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd333 = vld1q_f32(t333 + i); /* extra */
    t333[i] = t333[i];
      /* skip scalar load */
      for (int t15 = 0; t15 < 1024; t15+=1) {
        int t334 = i;
        int t335 = t334 * 1024;
        int t336 = t335 + t15;
        float t337 = memory[2631680 + t336];
        float t338 = t337 + t333[i];
        int t339 = i;
        int t340 = t339 * 1024;
        int t341 = t340 + t15;
        memory[3155968 + t341] = t338;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t16 = 0; t16 < 1024; t16+=1) {
        int t344 = i;
        int t345 = t344 * 1024;
        int t346 = t345 + t16;
        float t347 = memory[2631680 + t346];
        float t348 = t347 * 6.283185;
        float t349 = cosf(t348);
        int t350 = i;
        int t351 = t350 * 1024;
        int t352 = t351 + t16;
        memory[3680256 + t352] = t349;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t17 = 0; t17 < 1024; t17+=1) {
        int t355 = i;
        int t356 = t355 * 1024;
        int t357 = t356 + t17;
        float t358 = memory[3680256 + t357];
        float t359 = t358 + 1.0;
        int t360 = i;
        int t361 = t360 * 1024;
        int t362 = t361 + t17;
        memory[4204544 + t362] = t359;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t18 = 0; t18 < 1024; t18+=1) {
        int t365 = i;
        int t366 = t365 * 1024;
        int t367 = t366 + t18;
        float t368 = memory[4204544 + t367];
        float t369 = t368 * 0.5;
        int t370 = i;
        int t371 = t370 * 1024;
        int t372 = t371 + t18;
        memory[3680256 + t372] = t369;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t19 = 0; t19 < 1024; t19+=1) {
        int t375 = i;
        int t376 = t375 * 1024;
        int t377 = t376 + t19;
        float t378 = memory[3155968 + t377];
        float t379 = t378 * 6.283185;
        float t380 = cosf(t379);
        int t381 = i;
        int t382 = t381 * 1024;
        int t383 = t382 + t19;
        memory[4204544 + t383] = t380;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t20 = 0; t20 < 1024; t20+=1) {
        int t386 = i;
        int t387 = t386 * 1024;
        int t388 = t387 + t20;
        float t389 = memory[4204544 + t388];
        float t390 = t389 + 1.0;
        int t391 = i;
        int t392 = t391 * 1024;
        int t393 = t392 + t20;
        memory[4728832 + t393] = t390;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t21 = 0; t21 < 1024; t21+=1) {
        int t396 = i;
        int t397 = t396 * 1024;
        int t398 = t397 + t21;
        float t399 = memory[4728832 + t398];
        float t400 = t399 * 0.5;
        int t401 = i;
        int t402 = t401 * 1024;
        int t403 = t402 + t21;
        memory[4204544 + t403] = t400;
      }
    }
  }
  float32x4_t simd69 = vld1q_f32(t69 + i); /* extra */
    t69[i] = t69[i];
  /* t407 declared globally */
  float32x4_t simd406 = vmulq_f32(simd69, c13);
  float32x4_t simd407 = vaddq_f32(simd406, c14); vst1q_f32(t407 + i, simd407);
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      t407[i] = t407[0];
      /* skip scalar load */
      for (int t22 = 0; t22 < 1024; t22+=1) {
        int t408 = i;
        int t409 = t408 * 1024;
        int t410 = t409 + t22;
        float t411 = memory[3680256 + t410];
        float t412 = powf(t411, t407[0]);
        int t413 = i;
        int t414 = t413 * 1024;
        int t415 = t414 + t22;
        memory[4728832 + t415] = t412;
        int t417 = i;
        int t418 = t417 * 1024;
        int t419 = t418 + t22;
        float t420 = memory[4204544 + t419];
        float t421 = powf(t420, t407[0]);
        int t422 = i;
        int t423 = t422 * 1024;
        int t424 = t423 + t22;
        memory[5253120 + t424] = t421;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t23 = 0; t23 < 1024; t23+=1) {
        int t427 = i;
        int t428 = t427 * 1024;
        int t429 = t428 + t23;
        float t430 = memory[3680256 + t429];
        float t431 = powf(t430, 18.0);
        int t432 = i;
        int t433 = t432 * 1024;
        int t434 = t433 + t23;
        memory[5777408 + t434] = t431;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t24 = 0; t24 < 1024; t24+=1) {
        int t437 = i;
        int t438 = t437 * 1024;
        int t439 = t438 + t24;
        float t440 = memory[4204544 + t439];
        float t441 = powf(t440, 18.0);
        int t442 = i;
        int t443 = t442 * 1024;
        int t444 = t443 + t24;
        memory[3680256 + t444] = t441;
      }
    }
  }
  /* skip scalar load */
  /* t447 declared globally */
  float32x4_t simd447 = vmulq_f32(simd69, c16); vst1q_f32(t447 + i, simd447);
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      t447[i] = t447[0];
      /* skip scalar load */
      for (int t25 = 0; t25 < 1024; t25+=1) {
        int t448 = i;
        int t449 = t448 * 1024;
        int t450 = t449 + t25;
        float t451 = memory[5777408 + t450];
        float t452 = t451 * t447[0];
        int t453 = i;
        int t454 = t453 * 1024;
        int t455 = t454 + t25;
        float t456 = memory[4728832 + t455];
        float t457 = t456 + t452;
        int t458 = i;
        int t459 = t458 * 1024;
        int t460 = t459 + t25;
        memory[4204544 + t460] = t457;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t26 = 0; t26 < 1024; t26+=1) {
        int t463 = i;
        int t464 = t463 * 1024;
        int t465 = t464 + t26;
        float t466 = memory[4204544 + t465];
        float t467 = fminf(t466, 1.0);
        int t468 = i;
        int t469 = t468 * 1024;
        int t470 = t469 + t26;
        memory[4728832 + t470] = t467;
      }
    }
  }
  /* skip scalar load */
  /* t473 declared globally */
  float32x4_t simd473 = vmulq_f32(simd69, c16); vst1q_f32(t473 + i, simd473);
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      t473[i] = t473[0];
      /* skip scalar load */
      for (int t27 = 0; t27 < 1024; t27+=1) {
        int t474 = i;
        int t475 = t474 * 1024;
        int t476 = t475 + t27;
        float t477 = memory[3680256 + t476];
        float t478 = t477 * t473[0];
        int t479 = i;
        int t480 = t479 * 1024;
        int t481 = t480 + t27;
        float t482 = memory[5253120 + t481];
        float t483 = t482 + t478;
        int t484 = i;
        int t485 = t484 * 1024;
        int t486 = t485 + t27;
        memory[4204544 + t486] = t483;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t28 = 0; t28 < 1024; t28+=1) {
        int t489 = i;
        int t490 = t489 * 1024;
        int t491 = t490 + t28;
        float t492 = memory[4204544 + t491];
        float t493 = fminf(t492, 1.0);
        int t494 = i;
        int t495 = t494 * 1024;
        int t496 = t495 + t28;
        memory[3680256 + t496] = t493;
      }
    }
  }
  float32x4_t simd74 = vld1q_f32(t74 + i); /* extra */
    t74[i] = t74[i];
  /* t499 declared globally */
  float32x4_t simd499 = vsubq_f32(c2, simd74); vst1q_f32(t499 + i, simd499);
  t499[i] = t499[0];
  /* skip scalar load */
  for (int simd29 = 0; simd29 < 1024; simd29+=4) {
    float32x4_t simd500 = vld1q_f32(&memory[3072 + (int)simd29]);
    float32x4_t simd501 = vmulq_f32(simd500, simd74);
    float32x4_t simd502 = vaddq_f32(simd501, simd499);
    vst1q_f32(&memory[2629632 + (int)simd29], simd502);
  }
  for (int i = 0; i < frameCount; i += 1) {
    /* t571 declared globally */
    t571[i] = memory[6829076];
    float t572 = t571[i] + 1.0;
    float t573 = 0.0 > 0.0f ? 0.0 : t572;
    float t574 = t573;
    float t575 = (t574 / 512.0f);
    float t576 = floorf(t575);
    float t577 = t576 * 512.0;
    float t578 = t573 - t577;
    memory[6829076] = t578;
    float t580 = t578 >= 512.0;
    if (t580) {
      float t582 = t578 - 512.0;
      memory[6829076] = t582;
    }
    if (0.0) {
      memory[6829076] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      float32x4_t simd332 = vld1q_f32(t332 + i); /* extra */
    t332[i] = t332[i];
      /* skip scalar load */
      float32x4_t simd68 = vld1q_f32(t68 + i); /* extra */
    t68[i] = t68[i];
      /* t505 declared globally */
      t505[i] = t68[i] * t332[i];
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd505 = vld1q_f32(t505 + i); /* extra */
    t505[i] = t505[i];
      /* skip scalar load */
      for (int t30 = 0; t30 < 1024; t30+=1) {
        float t506 = memory[2629632 + t30];
        float t507 = t506 * t505[i];
        int t508 = i;
        int t509 = t508 * 1024;
        int t510 = t509 + t30;
        float t511 = memory[4728832 + t510];
        float t512 = t507 * t511;
        int t513 = i;
        int t514 = t513 * 1024;
        int t515 = t514 + t30;
        memory[4204544 + t515] = t512;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t31 = 0; t31 < 1024; t31+=1) {
        int t518 = i;
        int t519 = t518 * 1024;
        int t520 = t519 + t31;
        float t521 = memory[4204544 + t520];
        float t522 = 1.0 - t521;
        int t523 = i;
        int t524 = t523 * 1024;
        int t525 = t524 + t31;
        memory[4728832 + t525] = t522;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t32 = 0; t32 < 1024; t32+=1) {
        int t528 = i;
        int t529 = t528 * 1024;
        int t530 = t529 + t32;
        float t531 = memory[4728832 + t530];
        float t532 = fmaxf(t531, 0.0);
        int t533 = i;
        int t534 = t533 * 1024;
        int t535 = t534 + t32;
        memory[4204544 + t535] = t532;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd332 = vld1q_f32(t332 + i); /* extra */
    t332[i] = t332[i];
      /* skip scalar load */
      float32x4_t simd68 = vld1q_f32(t68 + i); /* extra */
    t68[i] = t68[i];
      /* t538 declared globally */
      t538[i] = t68[i] * t332[i];
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      float32x4_t simd538 = vld1q_f32(t538 + i); /* extra */
    t538[i] = t538[i];
      /* skip scalar load */
      for (int t33 = 0; t33 < 1024; t33+=1) {
        float t539 = memory[2629632 + t33];
        float t540 = t539 * t538[i];
        int t541 = i;
        int t542 = t541 * 1024;
        int t543 = t542 + t33;
        float t544 = memory[3680256 + t543];
        float t545 = t540 * t544;
        int t546 = i;
        int t547 = t546 * 1024;
        int t548 = t547 + t33;
        memory[4728832 + t548] = t545;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t34 = 0; t34 < 1024; t34+=1) {
        int t551 = i;
        int t552 = t551 * 1024;
        int t553 = t552 + t34;
        float t554 = memory[4728832 + t553];
        float t555 = 1.0 - t554;
        int t556 = i;
        int t557 = t556 * 1024;
        int t558 = t557 + t34;
        memory[3680256 + t558] = t555;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t35 = 0; t35 < 1024; t35+=1) {
        int t561 = i;
        int t562 = t561 * 1024;
        int t563 = t562 + t35;
        float t564 = memory[3680256 + t563];
        float t565 = fmaxf(t564, 0.0);
        int t566 = i;
        int t567 = t566 * 1024;
        int t568 = t567 + t35;
        memory[4728832 + t568] = t565;
      }
    }
    float32x4_t simd571 = vld1q_f32(t571 + i); /* extra */
    t571[i] = t571[i];
    if (t571[i] == 0.0f) {
      /* skip scalar load */
      float t588 = t571[i] == 0.0;
      if (t588) {
        for (int t590 = 0; t590 < 1024; t590++) {
          float t591 = ({
    uint32_t s; memcpy(&s, &memory[6829077], sizeof(uint32_t));
    if (s == 0u) s = 1u;
    s ^= s << 13; s ^= s >> 17; s ^= s << 5;
    memcpy(&memory[6829077], &s, sizeof(uint32_t));
    (float)s / 4294967296.0f;
});
          float t592 = t591 * 2.0;
          float t593 = t592 - 1.0;
          memory[6829078 + (int)t590] = t593;
        }
      }
    }
    /* skip scalar load */
    if (t571[i] == 0.0f) {
      /* skip scalar load */
      for (int t37 = 0; t37 < 1024; t37+=1) {
        float t597 = memory[6829078 + t37];
        float t598 = fabs(t597);
        int t599 = i;
        int t600 = t599 * 1024;
        int t601 = t600 + t37;
        memory[3680256 + t601] = t598;
      }
    }
  }
  float32x4_t simd77 = vld1q_f32(t77 + i); /* extra */
    t77[i] = t77[i];
  /* t605 declared globally */
  float32x4_t simd604 = vmulq_f32(simd77, c18);
  float32x4_t simd605 = vaddq_f32(simd604, c19); vst1q_f32(t605 + i, simd605);
  for (int i = 0; i < frameCount; i += 1) {
    /* t616 declared globally */
    t616[i] = memory[6830102];
    float t617 = t616[i] + 1.0;
    float t618 = 0.0 > 0.0f ? 0.0 : t617;
    float t619 = t618;
    float t620 = (t619 / 512.0f);
    float t621 = floorf(t620);
    float t622 = t621 * 512.0;
    float t623 = t618 - t622;
    memory[6830102] = t623;
    float t625 = t623 >= 512.0;
    if (t625) {
      float t627 = t623 - 512.0;
      memory[6830102] = t627;
    }
    if (0.0) {
      memory[6830102] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd571 = vld1q_f32(t571 + i); /* extra */
    t571[i] = t571[i];
    if (t571[i] == 0.0f) {
      t605[i] = t605[0];
      /* skip scalar load */
      for (int t38 = 0; t38 < 1024; t38+=1) {
        int t606 = i;
        int t607 = t606 * 1024;
        int t608 = t607 + t38;
        float t609 = memory[3680256 + t608];
        float t610 = powf(t609, t605[0]);
        int t611 = i;
        int t612 = t611 * 1024;
        int t613 = t612 + t38;
        memory[5253120 + t613] = t610;
      }
    }
    float32x4_t simd616 = vld1q_f32(t616 + i); /* extra */
    t616[i] = t616[i];
    if (t616[i] == 0.0f) {
      /* skip scalar load */
      float t633 = t616[i] == 0.0;
      if (t633) {
        for (int t635 = 0; t635 < 1024; t635++) {
          float t636 = ({
    uint32_t s; memcpy(&s, &memory[6830103], sizeof(uint32_t));
    if (s == 0u) s = 1u;
    s ^= s << 13; s ^= s >> 17; s ^= s << 5;
    memcpy(&memory[6830103], &s, sizeof(uint32_t));
    (float)s / 4294967296.0f;
});
          float t637 = t636 * 2.0;
          float t638 = t637 - 1.0;
          memory[6830104 + (int)t635] = t638;
        }
      }
    }
    /* skip scalar load */
    if (t616[i] == 0.0f) {
      /* skip scalar load */
      for (int t40 = 0; t40 < 1024; t40+=1) {
        float t642 = memory[6830104 + t40];
        float t643 = fabs(t642);
        int t644 = i;
        int t645 = t644 * 1024;
        int t646 = t645 + t40;
        memory[3680256 + t646] = t643;
      }
    }
  }
  /* skip scalar load */
  /* t650 declared globally */
  float32x4_t simd649 = vmulq_f32(simd77, c18);
  float32x4_t simd650 = vaddq_f32(simd649, c19); vst1q_f32(t650 + i, simd650);
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd616 = vld1q_f32(t616 + i); /* extra */
    t616[i] = t616[i];
    if (t616[i] == 0.0f) {
      t650[i] = t650[0];
      /* skip scalar load */
      for (int t41 = 0; t41 < 1024; t41+=1) {
        int t651 = i;
        int t652 = t651 * 1024;
        int t653 = t652 + t41;
        float t654 = memory[3680256 + t653];
        float t655 = powf(t654, t650[0]);
        int t656 = i;
        int t657 = t656 * 1024;
        int t658 = t657 + t41;
        memory[5777408 + t658] = t655;
      }
    }
  }
  /* skip scalar load */
  /* t665 declared globally */
  /* t664 declared globally */
  /* t662 declared globally */
  float32x4_t simd661 = vmulq_f32(simd77, c20);
  float32x4_t simd662 = vsubq_f32(c2, simd661); vst1q_f32(t662 + i, simd662);
  float32x4_t simd663 = vsubq_f32(c2, simd77);
  float32x4_t simd664 = simd663; vst1q_f32(t664 + i, simd664);
  float32x4_t simd665 = vsubq_f32(c2, simd662); vst1q_f32(t665 + i, simd665);
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd571 = vld1q_f32(t571 + i); /* extra */
    t571[i] = t571[i];
    if (t571[i] == 0.0f) {
      t665[i] = t665[0];
      t664[i] = t664[0];
      t662[i] = t662[0];
      /* skip scalar load */
      /* skip scalar load */
      for (int t42 = 0; t42 < 1024; t42+=1) {
        int t666 = i;
        int t667 = t666 * 1024;
        int t668 = t667 + t42;
        float t669 = memory[5253120 + t668];
        float t670 = t669 * t665[0];
        float t671 = t670 + t662[0];
        float t672 = t671 * t77[i];
        float t673 = t672 + t664[0];
        int t674 = i;
        int t675 = t674 * 1024;
        int t676 = t675 + t42;
        memory[3680256 + t676] = t673;
      }
    }
  }
  t662[i] = t662[0];
  /* skip scalar load */
  /* t681 declared globally */
  /* t680 declared globally */
  float32x4_t simd679 = vsubq_f32(c2, simd77);
  float32x4_t simd680 = simd679; vst1q_f32(t680 + i, simd680);
  float32x4_t simd681 = vsubq_f32(c2, simd662); vst1q_f32(t681 + i, simd681);
  for (int i = 0; i < frameCount; i += 1) {
    /* t735 declared globally */
    t735[i] = memory[6831128];
    float t736 = t735[i] + 1.0;
    float t737 = 0.0 > 0.0f ? 0.0 : t736;
    float t738 = t737;
    float t739 = (t738 / 512.0f);
    float t740 = floorf(t739);
    float t741 = t740 * 512.0;
    float t742 = t737 - t741;
    memory[6831128] = t742;
    float t744 = t742 >= 512.0;
    if (t744) {
      float t746 = t742 - 512.0;
      memory[6831128] = t746;
    }
    if (0.0) {
      memory[6831128] = 0.0;
    }
    /* t768 declared globally */
    t768[i] = memory[6832154];
    float t769 = t768[i] + 1.0;
    float t770 = 0.0 > 0.0f ? 0.0 : t769;
    float t771 = t770;
    float t772 = (t771 / 512.0f);
    float t773 = floorf(t772);
    float t774 = t773 * 512.0;
    float t775 = t770 - t774;
    memory[6832154] = t775;
    float t777 = t775 >= 512.0;
    if (t777) {
      float t779 = t775 - 512.0;
      memory[6832154] = t779;
    }
    if (0.0) {
      memory[6832154] = 0.0;
    }
  }
  for (int i = 0; i < frameCount; i += 1) {
    float32x4_t simd616 = vld1q_f32(t616 + i); /* extra */
    t616[i] = t616[i];
    if (t616[i] == 0.0f) {
      t681[i] = t681[0];
      t680[i] = t680[0];
      /* skip scalar load */
      /* skip scalar load */
      /* skip scalar load */
      for (int t43 = 0; t43 < 1024; t43+=1) {
        int t682 = i;
        int t683 = t682 * 1024;
        int t684 = t683 + t43;
        float t685 = memory[5777408 + t684];
        float t686 = t685 * t681[0];
        float t687 = t686 + t662[0];
        float t688 = t687 * t77[i];
        float t689 = t688 + t680[0];
        int t690 = i;
        int t691 = t690 * 1024;
        int t692 = t691 + t43;
        memory[5253120 + t692] = t689;
      }
    }
    float32x4_t simd104 = vld1q_f32(t104 + i); /* extra */
    t104[i] = t104[i];
    if (t104[i] == 0.0f) {
      float32x4_t simd162 = vld1q_f32(t162 + i); /* extra */
    t162[i] = t162[i];
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      float32x4_t simd76 = vld1q_f32(t76 + i); /* extra */
    t76[i] = t76[i];
      for (int t44 = 0; t44 < 1024; t44+=1) {
        int t695 = i;
        int t696 = t695 * 1024;
        int t697 = t696 + t44;
        float t698 = memory[529408 + t697];
        int t699 = i;
        int t700 = t699 * 1024;
        int t701 = t700 + t44;
        float t702 = memory[4204544 + t701];
        float t703 = t698 * t702;
        int t704 = i;
        int t705 = t704 * 1024;
        int t706 = t705 + t44;
        float t707 = memory[3680256 + t706];
        float t708 = t703 * t707;
        int t709 = i;
        int t710 = t709 * 1024;
        int t711 = t710 + t44;
        memory[5777408 + t711] = t708;
        int t713 = i;
        int t714 = t713 * 1024;
        int t715 = t714 + t44;
        float t716 = memory[1581056 + t715];
        int t717 = i;
        int t718 = t717 * 1024;
        int t719 = t718 + t44;
        float t720 = memory[4728832 + t719];
        float t721 = t716 * t720;
        int t722 = i;
        int t723 = t722 * 1024;
        int t724 = t723 + t44;
        float t725 = memory[5253120 + t724];
        float t726 = t721 * t725;
        int t727 = i;
        int t728 = t727 * 1024;
        int t729 = t728 + t44;
        memory[6301696 + t729] = t726;
        float t731 = memory[2629632 + t44];
        float t732 = t731 * t76[i];
        memory[2630656 + t44] = t732;
      }
    }
    float32x4_t simd735 = vld1q_f32(t735 + i); /* extra */
    t735[i] = t735[i];
    if (t735[i] == 0.0f) {
      /* skip scalar load */
      float t752 = t735[i] == 0.0;
      if (t752) {
        for (int t754 = 0; t754 < 1024; t754++) {
          float t755 = ({
    uint32_t s; memcpy(&s, &memory[6831129], sizeof(uint32_t));
    if (s == 0u) s = 1u;
    s ^= s << 13; s ^= s >> 17; s ^= s << 5;
    memcpy(&memory[6831129], &s, sizeof(uint32_t));
    (float)s / 4294967296.0f;
});
          float t756 = t755 * 2.0;
          float t757 = t756 - 1.0;
          memory[6831130 + (int)t754] = t757;
        }
      }
    }
    /* skip scalar load */
    if (t735[i] == 0.0f) {
      /* skip scalar load */
      for (int t46 = 0; t46 < 1024; t46+=1) {
        float t761 = memory[6831130 + t46];
        float t762 = t761 * 6.283185;
        int t763 = i;
        int t764 = t763 * 1024;
        int t765 = t764 + t46;
        memory[529408 + t765] = t762;
      }
    }
    float32x4_t simd768 = vld1q_f32(t768 + i); /* extra */
    t768[i] = t768[i];
    if (t768[i] == 0.0f) {
      /* skip scalar load */
      float t785 = t768[i] == 0.0;
      if (t785) {
        for (int t787 = 0; t787 < 1024; t787++) {
          float t788 = ({
    uint32_t s; memcpy(&s, &memory[6832155], sizeof(uint32_t));
    if (s == 0u) s = 1u;
    s ^= s << 13; s ^= s >> 17; s ^= s << 5;
    memcpy(&memory[6832155], &s, sizeof(uint32_t));
    (float)s / 4294967296.0f;
});
          float t789 = t788 * 2.0;
          float t790 = t789 - 1.0;
          memory[6832156 + (int)t787] = t790;
        }
      }
    }
    /* skip scalar load */
    if (t768[i] == 0.0f) {
      /* skip scalar load */
      for (int t48 = 0; t48 < 1024; t48+=1) {
        float t794 = memory[6832156 + t48];
        float t795 = t794 * 6.283185;
        int t796 = i;
        int t797 = t796 * 1024;
        int t798 = t797 + t48;
        memory[1581056 + t798] = t795;
      }
    }
    float32x4_t simd291 = vld1q_f32(t291 + i); /* extra */
    t291[i] = t291[i];
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t49 = 0; t49 < 1024; t49+=1) {
        int t801 = i;
        int t802 = t801 * 1024;
        int t803 = t802 + t49;
        float t804 = memory[2631680 + t803];
        float t805 = t804 * 6.283185;
        int t806 = i;
        int t807 = t806 * 1024;
        int t808 = t807 + t49;
        memory[3680256 + t808] = t805;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      for (int t50 = 0; t50 < 1024; t50+=1) {
        int t811 = i;
        int t812 = t811 * 1024;
        int t813 = t812 + t50;
        float t814 = memory[3155968 + t813];
        float t815 = t814 * 6.283185;
        int t816 = i;
        int t817 = t816 * 1024;
        int t818 = t817 + t50;
        memory[2631680 + t818] = t815;
      }
    }
    /* skip scalar load */
    if (t735[i] == 0.0f) {
      /* skip scalar load */
      for (int t51 = 0; t51 < 1024; t51+=1) {
        int t821 = i;
        int t822 = t821 * 1024;
        int t823 = t822 + t51;
        float t824 = memory[529408 + t823];
        float t825 = t824 * 0.65;
        int t826 = i;
        int t827 = t826 * 1024;
        int t828 = t827 + t51;
        memory[3155968 + t828] = t825;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      for (int t52 = 0; t52 < 1024; t52+=1) {
        int t831 = i;
        int t832 = t831 * 1024;
        int t833 = t832 + t52;
        float t834 = memory[3680256 + t833];
        float t835 = t834 * 0.35;
        int t836 = i;
        int t837 = t836 * 1024;
        int t838 = t837 + t52;
        float t839 = memory[3155968 + t838];
        float t840 = t839 + t835;
        int t841 = i;
        int t842 = t841 * 1024;
        int t843 = t842 + t52;
        memory[529408 + t843] = t840;
      }
    }
    /* skip scalar load */
    if (t768[i] == 0.0f) {
      /* skip scalar load */
      for (int t53 = 0; t53 < 1024; t53+=1) {
        int t846 = i;
        int t847 = t846 * 1024;
        int t848 = t847 + t53;
        float t849 = memory[1581056 + t848];
        float t850 = t849 * 0.65;
        int t851 = i;
        int t852 = t851 * 1024;
        int t853 = t852 + t53;
        memory[3155968 + t853] = t850;
      }
    }
    /* skip scalar load */
    if (t291[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      for (int t54 = 0; t54 < 1024; t54+=1) {
        int t856 = i;
        int t857 = t856 * 1024;
        int t858 = t857 + t54;
        float t859 = memory[2631680 + t858];
        float t860 = t859 * 0.35;
        int t861 = i;
        int t862 = t861 * 1024;
        int t863 = t862 + t54;
        float t864 = memory[3155968 + t863];
        float t865 = t864 + t860;
        int t866 = i;
        int t867 = t866 * 1024;
        int t868 = t867 + t54;
        memory[1581056 + t868] = t865;
      }
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t55 = 0; t55 < 1024; t55+=1) {
        float t871 = memory[2630656 + t55];
        float t872 = 1.0 - t871;
        int t873 = i;
        int t874 = t873 * 1024;
        int t875 = t874 + t55;
        float t876 = memory[1056768 + t875];
        float t877 = t876 * t872;
        int t878 = i;
        int t879 = t878 * 1024;
        int t880 = t879 + t55;
        float t881 = memory[529408 + t880];
        float t882 = memory[2630656 + t55];
        float t883 = t881 * t882;
        float t884 = t877 + t883;
        int t885 = i;
        int t886 = t885 * 1024;
        int t887 = t886 + t55;
        memory[2631680 + t887] = t884;
      }
    }
    float32x4_t simd162 = vld1q_f32(t162 + i); /* extra */
    t162[i] = t162[i];
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t56 = 0; t56 < 1024; t56+=1) {
        float t890 = memory[2630656 + t56];
        float t891 = 1.0 - t890;
        int t892 = i;
        int t893 = t892 * 1024;
        int t894 = t893 + t56;
        float t895 = memory[2105344 + t894];
        float t896 = t895 * t891;
        int t897 = i;
        int t898 = t897 * 1024;
        int t899 = t898 + t56;
        float t900 = memory[1581056 + t899];
        float t901 = memory[2630656 + t56];
        float t902 = t900 * t901;
        float t903 = t896 + t902;
        int t904 = i;
        int t905 = t904 * 1024;
        int t906 = t905 + t56;
        float t907 = memory[2631680 + t906];
        float t908 = cosf(t907);
        int t909 = i;
        int t910 = t909 * 1024;
        int t911 = t910 + t56;
        float t912 = memory[5777408 + t911];
        float t913 = t912 * t908;
        int t914 = i;
        int t915 = t914 * 1024;
        int t916 = t915 + t56;
        memory[529408 + t916] = t913;
        int t918 = i;
        int t919 = t918 * 1024;
        int t920 = t919 + t56;
        float t921 = memory[2631680 + t920];
        float t922 = sinf(t921);
        int t923 = i;
        int t924 = t923 * 1024;
        int t925 = t924 + t56;
        float t926 = memory[5777408 + t925];
        float t927 = t926 * t922;
        int t928 = i;
        int t929 = t928 * 1024;
        int t930 = t929 + t56;
        memory[1056768 + t930] = t927;
        float t932 = cosf(t903);
        int t933 = i;
        int t934 = t933 * 1024;
        int t935 = t934 + t56;
        float t936 = memory[6301696 + t935];
        float t937 = t936 * t932;
        int t938 = i;
        int t939 = t938 * 1024;
        int t940 = t939 + t56;
        memory[3155968 + t940] = t937;
        float t942 = sinf(t903);
        int t943 = i;
        int t944 = t943 * 1024;
        int t945 = t944 + t56;
        float t946 = memory[6301696 + t945];
        float t947 = t946 * t942;
        int t948 = i;
        int t949 = t948 * 1024;
        int t950 = t949 + t56;
        memory[3680256 + t950] = t947;
      }
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t953 = 0; t953 < 1024; t953++) {
        float t954 = memory[1053696 + (isfinite((int) t953) ? (int) t953 : 0)];
        float t955 = memory[1054720 + (isfinite((int) t953) ? (int) t953 : 0)];
        memory[2629632 + (int)t953] = t954;
        memory[2630656 + (int)t953] = t955;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[2629632], .imagp = &memory[2630656] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Inverse);
}
      for (int t960 = 0; t960 < 1024; t960++) {
        float t961 = memory[2629632 + (isfinite((int) t960) ? (int) t960 : 0)];
        float t962 = t961 * 0.0009765625;
        memory[2629632 + (int)t960] = t962;
      }
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      for (int t965 = 0; t965 < 1024; t965++) {
        float t966 = memory[5120 + (isfinite((int) t965) ? (int) t965 : 0)];
        float t967 = memory[1055744 + (isfinite((int) t965) ? (int) t965 : 0)];
        memory[1053696 + (int)t965] = t966;
        memory[1054720 + (int)t965] = t967;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[1053696], .imagp = &memory[1054720] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Inverse);
}
      for (int t972 = 0; t972 < 1024; t972++) {
        float t973 = memory[1053696 + (isfinite((int) t972) ? (int) t972 : 0)];
        float t974 = t973 * 0.0009765625;
        memory[1053696 + (int)t972] = t974;
      }
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t977 = 0; t977 < 1024; t977++) {
        int t978 = i;
        int t979 = t978 * 1024;
        int t980 = t979 + t977;
        float t981 = memory[529408 + t980];
        int t982 = i;
        int t983 = t982 * 1024;
        int t984 = t983 + t977;
        float t985 = memory[1056768 + t984];
        memory[1054720 + (int)t977] = t981;
        memory[1055744 + (int)t977] = t985;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[1054720], .imagp = &memory[1055744] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Inverse);
}
      for (int t990 = 0; t990 < 1024; t990++) {
        float t991 = memory[1054720 + (isfinite((int) t990) ? (int) t990 : 0)];
        float t992 = t991 * 0.0009765625;
        memory[1054720 + (int)t990] = t992;
      }
    }
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      for (int t995 = 0; t995 < 1024; t995++) {
        int t996 = i;
        int t997 = t996 * 1024;
        int t998 = t997 + t995;
        float t999 = memory[3155968 + t998];
        int t1000 = i;
        int t1001 = t1000 * 1024;
        int t1002 = t1001 + t995;
        float t1003 = memory[3680256 + t1002];
        memory[1055744 + (int)t995] = t999;
        memory[2630656 + (int)t995] = t1003;
      }
      {
  static FFTSetup _dgen_fft_setup_10 = NULL;
  if (_dgen_fft_setup_10 == NULL) {
    _dgen_fft_setup_10 = vDSP_create_fftsetup(10, kFFTRadix2);
  }
  DSPSplitComplex _dgen_sc = { .realp = &memory[1055744], .imagp = &memory[2630656] };
  vDSP_fft_zip(_dgen_fft_setup_10, &_dgen_sc, 1, 10, kFFTDirection_Inverse);
}
      for (int t1008 = 0; t1008 < 1024; t1008++) {
        float t1009 = memory[1055744 + (isfinite((int) t1008) ? (int) t1008 : 0)];
        float t1010 = t1009 * 0.0009765625;
        memory[1055744 + (int)t1008] = t1010;
      }
    }
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      /* skip scalar load */
      float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
      for (int t60 = 0; t60 < 1024; t60+=1) {
        float t1013 = memory[2629632 + t60];
        float t1014 = memory[4096 + t60];
        float t1015 = t1013 * t1014;
        int t1016 = i;
        int t1017 = t1016 * 1024;
        int t1018 = t1017 + t60;
        memory[5120 + t1018] = t1015;
      }
    }
    float32x4_t simd102 = vld1q_f32(t102 + i); /* extra */
    t102[i] = t102[i];
    /* t1041 declared globally */
    float t1021 = memory[6834204 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1022 = memory[6834205 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1023 = t1022 == 0.0;
    if (t1023) {
      for (int t1025 = 0; t1025 < 1024; t1025++) {
        int t1026 = i;
        int t1027 = t1026 * 1024;
        int t1028 = t1027 + t1025;
        float t1029 = memory[5120 + t1028];
        float t1030 = t1021 + t1025;
        float t1031 = t1030 >= 1024.0;
        float t1032 = t1030 - 1024.0;
        float t1033 = t1031 > 0.0f ? t1032 : t1030;
        float t1034 = (int)t1033;
        float t1035 = memory[6833180 + (isfinite((int) t1034) ? (int) t1034 : 0)];
        float t1036 = t1035 + t1029;
        memory[6833180 + (int)t1034] = t1036;
      }
    }
    float t1040 = (int)t1021;
    t1041[i] = memory[6833180 + (isfinite((int) t1040) ? (int) t1040 : 0)];
    float t1042 = (int)t1021;
    memory[6833180 + (int)t1042] = 0.0;
    float t1044 = t1021 + 1.0;
    float t1045 = t1044 >= 1024.0;
    float t1046 = t1045 > 0.0f ? 0.0 : t1044;
    memory[6834204 + (int)0.0] = t1046;
    float t1048 = t1022 + 1.0;
    float t1049 = t1048 >= 512.0;
    float t1050 = t1049 > 0.0f ? 0.0 : t1048;
    memory[6834205 + (int)0.0] = t1050;
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
      for (int t61 = 0; t61 < 1024; t61+=1) {
        float t1052 = memory[1053696 + t61];
        float t1053 = memory[4096 + t61];
        float t1054 = t1052 * t1053;
        int t1055 = i;
        int t1056 = t1055 * 1024;
        int t1057 = t1056 + t61;
        memory[5120 + t1057] = t1054;
      }
    }
    float32x4_t simd160 = vld1q_f32(t160 + i); /* extra */
    t160[i] = t160[i];
    /* t1080 declared globally */
    float t1060 = memory[6835230 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1061 = memory[6835231 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1062 = t1061 == 0.0;
    if (t1062) {
      for (int t1064 = 0; t1064 < 1024; t1064++) {
        int t1065 = i;
        int t1066 = t1065 * 1024;
        int t1067 = t1066 + t1064;
        float t1068 = memory[5120 + t1067];
        float t1069 = t1060 + t1064;
        float t1070 = t1069 >= 1024.0;
        float t1071 = t1069 - 1024.0;
        float t1072 = t1070 > 0.0f ? t1071 : t1069;
        float t1073 = (int)t1072;
        float t1074 = memory[6834206 + (isfinite((int) t1073) ? (int) t1073 : 0)];
        float t1075 = t1074 + t1068;
        memory[6834206 + (int)t1073] = t1075;
      }
    }
    float t1079 = (int)t1060;
    t1080[i] = memory[6834206 + (isfinite((int) t1079) ? (int) t1079 : 0)];
    float t1081 = (int)t1060;
    memory[6834206 + (int)t1081] = 0.0;
    float t1083 = t1060 + 1.0;
    float t1084 = t1083 >= 1024.0;
    float t1085 = t1084 > 0.0f ? 0.0 : t1083;
    memory[6835230 + (int)0.0] = t1085;
    float t1087 = t1061 + 1.0;
    float t1088 = t1087 >= 512.0;
    float t1089 = t1088 > 0.0f ? 0.0 : t1087;
    memory[6835231 + (int)0.0] = t1089;
    /* skip scalar load */
    if (t104[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      for (int t62 = 0; t62 < 1024; t62+=1) {
        float t1091 = memory[1054720 + t62];
        float t1092 = memory[4096 + t62];
        float t1093 = t1091 * t1092;
        int t1094 = i;
        int t1095 = t1094 * 1024;
        int t1096 = t1095 + t62;
        memory[5120 + t1096] = t1093;
      }
    }
    /* skip scalar load */
    /* t1119 declared globally */
    float t1099 = memory[6836256 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1100 = memory[6836257 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1101 = t1100 == 0.0;
    if (t1101) {
      for (int t1103 = 0; t1103 < 1024; t1103++) {
        int t1104 = i;
        int t1105 = t1104 * 1024;
        int t1106 = t1105 + t1103;
        float t1107 = memory[5120 + t1106];
        float t1108 = t1099 + t1103;
        float t1109 = t1108 >= 1024.0;
        float t1110 = t1108 - 1024.0;
        float t1111 = t1109 > 0.0f ? t1110 : t1108;
        float t1112 = (int)t1111;
        float t1113 = memory[6835232 + (isfinite((int) t1112) ? (int) t1112 : 0)];
        float t1114 = t1113 + t1107;
        memory[6835232 + (int)t1112] = t1114;
      }
    }
    float t1118 = (int)t1099;
    t1119[i] = memory[6835232 + (isfinite((int) t1118) ? (int) t1118 : 0)];
    float t1120 = (int)t1099;
    memory[6835232 + (int)t1120] = 0.0;
    float t1122 = t1099 + 1.0;
    float t1123 = t1122 >= 1024.0;
    float t1124 = t1123 > 0.0f ? 0.0 : t1122;
    memory[6836256 + (int)0.0] = t1124;
    float t1126 = t1100 + 1.0;
    float t1127 = t1126 >= 512.0;
    float t1128 = t1127 > 0.0f ? 0.0 : t1126;
    memory[6836257 + (int)0.0] = t1128;
    /* skip scalar load */
    if (t162[i] == 0.0f) {
      /* skip scalar load */
      /* skip scalar load */
      for (int t63 = 0; t63 < 1024; t63+=1) {
        float t1130 = memory[1055744 + t63];
        float t1131 = memory[4096 + t63];
        float t1132 = t1130 * t1131;
        int t1133 = i;
        int t1134 = t1133 * 1024;
        int t1135 = t1134 + t63;
        memory[5120 + t1135] = t1132;
      }
    }
    float32x4_t simd1119 = vld1q_f32(t1119 + i); /* extra */
    t1119[i] = t1119[i];
    float32x4_t simd1080 = vld1q_f32(t1080 + i); /* extra */
    t1080[i] = t1080[i];
    float32x4_t simd1041 = vld1q_f32(t1041 + i); /* extra */
    t1041[i] = t1041[i];
    /* skip scalar load */
    /* skip scalar load */
    float32x4_t simd80 = vld1q_f32(t80 + i); /* extra */
    t80[i] = t80[i];
    float32x4_t simd79 = vld1q_f32(t79 + i); /* extra */
    t79[i] = t79[i];
    float32x4_t simd78 = vld1q_f32(t78 + i); /* extra */
    t78[i] = t78[i];
    float t1138 = memory[6837282 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1139 = memory[6837283 + (isfinite((int) 0.0) ? (int) 0.0 : 0)];
    float t1140 = t1139 == 0.0;
    if (t1140) {
      for (int t1142 = 0; t1142 < 1024; t1142++) {
        int t1143 = i;
        int t1144 = t1143 * 1024;
        int t1145 = t1144 + t1142;
        float t1146 = memory[5120 + t1145];
        float t1147 = t1138 + t1142;
        float t1148 = t1147 >= 1024.0;
        float t1149 = t1147 - 1024.0;
        float t1150 = t1148 > 0.0f ? t1149 : t1147;
        float t1151 = (int)t1150;
        float t1152 = memory[6836258 + (isfinite((int) t1151) ? (int) t1151 : 0)];
        float t1153 = t1152 + t1146;
        memory[6836258 + (int)t1151] = t1153;
      }
    }
    float t1157 = (int)t1138;
    float t1158 = memory[6836258 + (isfinite((int) t1157) ? (int) t1157 : 0)];
    float t1159 = (int)t1138;
    memory[6836258 + (int)t1159] = 0.0;
    float t1161 = t1138 + 1.0;
    float t1162 = t1161 >= 1024.0;
    float t1163 = t1162 > 0.0f ? 0.0 : t1161;
    memory[6837282 + (int)0.0] = t1163;
    float t1165 = t1139 + 1.0;
    float t1166 = t1165 >= 512.0;
    float t1167 = t1166 > 0.0f ? 0.0 : t1165;
    memory[6837283 + (int)0.0] = t1167;
    float t1169 = 1.0 - t79[i];
    float t1170 = t1041[i] * t1169;
    float t1171 = t1119[i] * t79[i];
    float t1172 = t1170 + t1171;
    float t1173 = t80[i] * t1172;
    float t1174 = 1.0 - t79[i];
    float t1175 = t1080[i] * t1174;
    float t1176 = t1158 * t79[i];
    float t1177 = t1175 + t1176;
    float t1178 = t80[i] * t1177;
    float t1179 = t1041[i] - t1119[i];
    float t1180 = t80[i] * t1179;
    float t1181 = t1080[i] - t1158;
    float t1182 = t80[i] * t1181;
    float t1183 = 1.0 - t78[i];
    float t1184 = t1173 * t1183;
    float t1185 = t1180 * t78[i];
    float t1186 = t1184 + t1185;
    float t1187 = 1.0 - t78[i];
    float t1188 = t1178 * t1187;
    float t1189 = t1182 * t78[i];
    float t1190 = t1188 + t1189;
    out[0][i] = sanitize_out_f32(t1186);
    out[1][i] = sanitize_out_f32(t1190);
  }
}