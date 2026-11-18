#!/usr/bin/env python3
"""Writes crates/iyakki-audiodec/src/opus_silk_tables.rs: the constant tables of the SILK decoder, copied as data from the reference
decoder source that RFC 6716 carries (BSD 3-clause, see legal/extra-licences/opus-reference). Usage: gen-silk-tables.py SILK_SRC_DIR OUT.rs"""
import re, sys
src, out = sys.argv[1], sys.argv[2]
files = ["tables_other.c","tables_gain.c","tables_LTP.c","table_LSF_cos.c","tables_pitch_lag.c","tables_pulses_per_block.c",
         "tables_NLSF_CB_NB_MB.c","tables_NLSF_CB_WB.c","pitch_est_tables.c","resampler_rom.c"]
text = ""
for f in files:
    t = open(f"{src}/{f}").read()
    t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)
    t = re.sub(r"//.*", "", t)
    text += t + "\n"
def grab(name):
    m = re.search(r"\b" + re.escape(name) + r"\s*(\[[^=]*\])\s*=\s*\{", text)
    if not m:
        m = re.search(r"\b" + re.escape(name) + r"\s*=\s*([-\d]+)\s*;", text)
        return None, [int(m.group(1))]
    i = m.end() - 1
    depth = 0
    for j in range(i, len(text)):
        if text[j] == "{": depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0: break
    body = text[i:j+1]
    toks = [x for x in re.split(r"[{},]", body) if x.strip()]
    vals = [int(eval(x.strip(), {"OFFSET_VL_Q10":32,"OFFSET_VH_Q10":100,"OFFSET_UVL_Q10":100,"OFFSET_UVH_Q10":240})) for x in toks]
    return m.group(1), vals
wanted = [
 ("silk_gain_iCDF","GAIN_ICDF","u8"),("silk_delta_gain_iCDF","DELTA_GAIN_ICDF","u8"),
 ("silk_LTP_per_index_iCDF","LTP_PER_INDEX_ICDF","u8"),("silk_LTP_gain_iCDF_0","LTP_GAIN_ICDF_0","u8"),
 ("silk_LTP_gain_iCDF_1","LTP_GAIN_ICDF_1","u8"),("silk_LTP_gain_iCDF_2","LTP_GAIN_ICDF_2","u8"),
 ("silk_LTP_gain_vq_0","LTP_GAIN_VQ_0","i8"),("silk_LTP_gain_vq_1","LTP_GAIN_VQ_1","i8"),("silk_LTP_gain_vq_2","LTP_GAIN_VQ_2","i8"),
 ("silk_LSFCosTab_FIX_Q12","LSF_COS_TAB_Q12","i16"),
 ("silk_stereo_pred_quant_Q13","STEREO_PRED_QUANT_Q13","i16"),("silk_stereo_pred_joint_iCDF","STEREO_PRED_JOINT_ICDF","u8"),
 ("silk_stereo_only_code_mid_iCDF","STEREO_ONLY_CODE_MID_ICDF","u8"),
 ("silk_LBRR_flags_2_iCDF","LBRR_FLAGS_2_ICDF","u8"),("silk_LBRR_flags_3_iCDF","LBRR_FLAGS_3_ICDF","u8"),
 ("silk_lsb_iCDF","LSB_ICDF","u8"),("silk_LTPscale_iCDF","LTP_SCALE_ICDF","u8"),
 ("silk_type_offset_VAD_iCDF","TYPE_OFFSET_VAD_ICDF","u8"),("silk_type_offset_no_VAD_iCDF","TYPE_OFFSET_NO_VAD_ICDF","u8"),
 ("silk_NLSF_interpolation_factor_iCDF","NLSF_INTERP_FACTOR_ICDF","u8"),
 ("silk_Quantization_Offsets_Q10","QUANTIZATION_OFFSETS_Q10","i16"),("silk_LTPScales_table_Q14","LTP_SCALES_Q14","i16"),
 ("silk_uniform3_iCDF","UNIFORM3_ICDF","u8"),("silk_uniform4_iCDF","UNIFORM4_ICDF","u8"),("silk_uniform5_iCDF","UNIFORM5_ICDF","u8"),
 ("silk_uniform6_iCDF","UNIFORM6_ICDF","u8"),("silk_uniform8_iCDF","UNIFORM8_ICDF","u8"),("silk_NLSF_EXT_iCDF","NLSF_EXT_ICDF","u8"),
 ("silk_pitch_lag_iCDF","PITCH_LAG_ICDF","u8"),("silk_pitch_delta_iCDF","PITCH_DELTA_ICDF","u8"),
 ("silk_pitch_contour_iCDF","PITCH_CONTOUR_ICDF","u8"),("silk_pitch_contour_NB_iCDF","PITCH_CONTOUR_NB_ICDF","u8"),
 ("silk_pitch_contour_10_ms_iCDF","PITCH_CONTOUR_10MS_ICDF","u8"),("silk_pitch_contour_10_ms_NB_iCDF","PITCH_CONTOUR_10MS_NB_ICDF","u8"),
 ("silk_pulses_per_block_iCDF","PULSES_PER_BLOCK_ICDF","u8"),("silk_rate_levels_iCDF","RATE_LEVELS_ICDF","u8"),
 ("silk_shell_code_table0","SHELL_CODE_TABLE0","u8"),("silk_shell_code_table1","SHELL_CODE_TABLE1","u8"),
 ("silk_shell_code_table2","SHELL_CODE_TABLE2","u8"),("silk_shell_code_table3","SHELL_CODE_TABLE3","u8"),
 ("silk_shell_code_table_offsets","SHELL_CODE_TABLE_OFFSETS","u8"),("silk_sign_iCDF","SIGN_ICDF","u8"),
 ("silk_NLSF_CB1_NB_MB_Q8","NLSF_CB1_NB_MB_Q8","u8"),("silk_NLSF_CB1_iCDF_NB_MB","NLSF_CB1_ICDF_NB_MB","u8"),
 ("silk_NLSF_CB2_SELECT_NB_MB","NLSF_CB2_SELECT_NB_MB","u8"),("silk_NLSF_CB2_iCDF_NB_MB","NLSF_CB2_ICDF_NB_MB","u8"),
 ("silk_NLSF_PRED_NB_MB_Q8","NLSF_PRED_NB_MB_Q8","u8"),("silk_NLSF_DELTA_MIN_NB_MB_Q15","NLSF_DELTA_MIN_NB_MB_Q15","i16"),
 ("silk_NLSF_CB1_WB_Q8","NLSF_CB1_WB_Q8","u8"),("silk_NLSF_CB1_iCDF_WB","NLSF_CB1_ICDF_WB","u8"),
 ("silk_NLSF_CB2_SELECT_WB","NLSF_CB2_SELECT_WB","u8"),("silk_NLSF_CB2_iCDF_WB","NLSF_CB2_ICDF_WB","u8"),
 ("silk_NLSF_PRED_WB_Q8","NLSF_PRED_WB_Q8","u8"),("silk_NLSF_DELTA_MIN_WB_Q15","NLSF_DELTA_MIN_WB_Q15","i16"),
 ("silk_CB_lags_stage2_10_ms","CB_LAGS_STAGE2_10MS","i8"),("silk_CB_lags_stage3_10_ms","CB_LAGS_STAGE3_10MS","i8"),
 ("silk_CB_lags_stage2","CB_LAGS_STAGE2","i8"),("silk_CB_lags_stage3","CB_LAGS_STAGE3","i8"),
 ("silk_resampler_up2_hq_0","RESAMPLER_UP2_HQ_0","i16"),("silk_resampler_up2_hq_1","RESAMPLER_UP2_HQ_1","i16"),
 ("silk_resampler_frac_FIR_12","RESAMPLER_FRAC_FIR_12","i16"),
]
o = ["//! Constant tables of the SILK decoder (RFC 6716 section 4.2), as data: probability tables (inverse cumulative form), codebooks of the",
     "//! line spectral frequencies, the long-term prediction codebooks, the pitch contours and the resampler filter taps.",
     "//! Taken from the tables of the standard's reference decoder (BSD licence, part of the RFC); only data, no code. Generated by",
     "//! `dist/opus/gen-silk-tables.py`; multi-dimensional tables are stored flat, row by row.", ""]
ty = {"u8":"u8","i8":"i8","i16":"i16"}
for cname, rname, t in wanted:
    dims, vals = grab(cname)
    o.append(f"pub const {rname}: [{t}; {len(vals)}] = [")
    for k in range(0, len(vals), 16):
        o.append("    " + ", ".join(str(v) for v in vals[k:k+16]) + ",")
    o.append("];\n")
open(out, "w").write("\n".join(o))
