#!/usr/bin/env python3
"""Export one language of AI4Bharat Indic-TTS (FastPitch + HiFi-GAN generator) to ONNX. Run on a PC, once per language.

  python3.10 -m venv venv && . venv/bin/activate
  pip install torch --index-url https://download.pytorch.org/whl/cpu     # CPU build is enough
  pip install onnx onnxruntime onnxscript numpy"<1.24" scipy "librosa==0.9.2" soundfile coqpit inflect unidecode anyascii pyyaml tqdm \
      pysbd einops "numba<0.60" matplotlib pandas trainer
  git clone --depth 1 https://github.com/gokulkarthik/TTS   # AI4Bharat's Coqui TTS fork (MPL-2.0; used only here, never shipped)
  pip install --no-deps -e TTS
  # the fork imports pyworld and MeCab at import time but never uses them for inference: empty stand-in modules are enough
  curl -LO https://github.com/AI4Bharat/Indic-TTS/releases/download/v1-checkpoints-release/ta.zip && unzip ta.zip -d models
  ln -s . models/v1        # the checkpoints' configs name their files models/v1/<lang>/...
  python export_onnx.py ta out/ta       # -> out/ta/{fastpitch.onnx, hifigan.onnx, vocab.json}

The exported graphs take the token ids of the text (see vocab.json), a speaker id and a duration scale, and make a mel-spectrogram;
the HiFi-GAN generator turns a mel-spectrogram into audio. The script checks the ONNX FastPitch against the PyTorch model on
two lengths. `models/<lang>` must hold fastpitch/ and hifigan/ as in the release archive. AI4Bharat's checkpoints have no stated
licence: see docs/LICENSES.md before you publish what this makes."""
import sys,json,os,numpy as np,torch
from TTS.utils.synthesizer import Synthesizer
from TTS.tts.utils.helpers import sequence_mask
lang,out=sys.argv[1],sys.argv[2]
os.makedirs(out,exist_ok=True)
m=f'models/{lang}'
s=Synthesizer(tts_checkpoint=f'{m}/fastpitch/best_model.pth',tts_config_path=f'{m}/fastpitch/config.json',tts_speakers_file=f'{m}/fastpitch/speakers.pth',vocoder_checkpoint=f'{m}/hifigan/best_model.pth',vocoder_config=f'{m}/hifigan/config.json',use_cuda=False)

import torch.nn as nn, math
def _mha(self, q, k, v, attn_mask=None, key_padding_mask=None, **kw):
    # single head, batch 1, no padding: export-friendly (no Python-int shapes baked in)
    assert self.num_heads == 1
    W, B = self.in_proj_weight, self.in_proj_bias
    d = W.shape[1]
    x = q.transpose(0, 1)  # [1,T,C]
    qq = torch.nn.functional.linear(x, W[:d], B[:d]); kk = torch.nn.functional.linear(x, W[d:2*d], B[d:2*d]); vv = torch.nn.functional.linear(x, W[2*d:], B[2*d:])
    sc = torch.matmul(qq, kk.transpose(1, 2)) / math.sqrt(d)
    a = torch.softmax(sc, dim=-1)
    o = torch.nn.functional.linear(torch.matmul(a, vv), self.out_proj.weight, self.out_proj.bias)
    return o.transpose(0, 1), torch.zeros(1, 1)
for mod in list(s.tts_model.modules()):
    if isinstance(mod, nn.MultiheadAttention):
        mod.forward = _mha.__get__(mod)

fp=s.tts_model.eval(); voc=s.vocoder_model.eval()
class FP(torch.nn.Module):
    def __init__(s_,m): super().__init__(); s_.m=m
    def forward(s_,x,spk,scale):
        m=s_.m
        g=spk
        xm=torch.ones_like(x).unsqueeze(1).to(torch.float32)
        o_en,xm,g,_=m._forward_encoder(x,xm,g)
        d=m.duration_predictor(o_en,xm)
        o_dr=(torch.exp(d)-1)*xm*scale
        o_dr=torch.where(o_dr<1,torch.ones_like(o_dr),o_dr)
        o_dr=torch.round(o_dr).squeeze(1)
        pe,_=m._forward_pitch_predictor(o_en,xm)
        o_en=o_en+pe
        idx=torch.repeat_interleave(torch.arange(o_en.shape[2]),o_dr.squeeze(0).to(torch.int64))
        o_ex=torch.index_select(o_en,2,idx)
        o_ex=m.pos_encoder(o_ex,None)
        o_de=m.decoder(o_ex,None,g=None).transpose(1,2)
        return o_de   # [1, T_mel, 80]
class VOC(torch.nn.Module):
    def __init__(s_,v): super().__init__(); s_.v=v
    def forward(s_,mel): return s_.v.inference(mel.transpose(1,2))
tok=fp.tokenizer if hasattr(fp,'tokenizer') else s.tts_model.tokenizer
ids=torch.tensor([tok.text_to_ids("ஒரு ஊரிலே ஒரு சிறிய முயல்.")]) if lang=='ta' else None
if ids is None:
    ids=torch.tensor([list(range(5,40))])
spk=torch.tensor([0]); sc=torch.tensor(1.0)
fpw=FP(fp).eval()
with torch.no_grad(): ref=fpw(ids,spk,sc)
print('mel',ref.shape)
torch.onnx.export(fpw,(ids,spk,sc),f'{out}/fastpitch.onnx',input_names=['tokens','speaker','scale'],output_names=['mel'],dynamic_axes={'tokens':{1:'T'},'mel':{1:'F'}},opset_version=17,dynamo=False)
# vocoder: Coqui inference takes [1,T,C]
import inspect; print(inspect.getsource(voc.inference))
vw=VOC(voc).eval()
with torch.no_grad(): wr=vw(ref)
print('wav',wr.shape)
torch.onnx.export(vw,(ref,),f'{out}/hifigan.onnx',input_names=['mel'],output_names=['wav'],dynamic_axes={'mel':{1:'F'},'wav':{2:'N'}},opset_version=17,dynamo=False)
# vocab & speakers
V={'tokens':{str(i):tok.characters.id_to_char(i) for i in range(tok.characters.num_chars)} if hasattr(tok.characters,'id_to_char') else None,
 'speakers':s.tts_model.speaker_manager.speaker_names,'speaker_ids':{k:int(v) for k,v in s.tts_model.speaker_manager.ids.items()},'sample_rate':int(s.output_sample_rate),'hop':256,'num_chars':tok.characters.num_chars,
 'config_characters':json.load(open(f'{m}/fastpitch/config.json'))['characters']}
json.dump(V,open(f'{out}/vocab.json','w'),ensure_ascii=False,indent=1)
print('speakers',V['speakers'])

import onnxruntime as ort
sess=ort.InferenceSession(f'{out}/fastpitch.onnx')
for n in (12,60):
    t=torch.tensor([[ (7+i*3)%(tok.characters.num_chars-5)+5 for i in range(n)]])
    with torch.no_grad(): r=fpw(t,spk,sc).numpy()
    o=sess.run(None,{'tokens':t.numpy(),'speaker':spk.numpy(),'scale':sc.numpy()})[0]
    print('VERIFY n',n,r.shape,o.shape,float(abs(r-o).max()) if r.shape==o.shape else 'SHAPE MISMATCH')
