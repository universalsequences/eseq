"""Check drive isolation and render a listening comparison at the new default register."""
import json
import numpy as np
import soundfile as sf
from scipy import signal
from verify import instrument,audio,OUT,HERE
inst=instrument(48000)
reel=[];rows=[]
for drive in [1,3,5]:
    p={'drive':drive,'clip_mix':.25,'air':0,'level':.4}
    dry=audio(inst,seconds=.8,pitch=440,params=p)
    wet=audio(inst,seconds=.8,pitch=440,params=p|{'air':2.5})
    noise=wet-dry
    if drive==1:reference_noise=noise;reference=dry
    assert np.max(abs(noise-reference_noise))<2e-6,'Drive must not clip or alter the air path'
    freq=np.fft.rfftfreq(len(dry),1/48000);z=abs(np.fft.rfft(dry))**2
    low=float(np.sum(z[(freq>25)&(freq<100)]))
    if drive==1:low_ref=low
    db=float(10*np.log10(low/low_ref));assert abs(db)<1,'Drive must preserve low-body energy'
    rows.append({'drive':drive,'low_body_delta_db':db,'peak':float(max(abs(wet)))})
    reel.extend([wet,np.zeros(12000)])
sf.write(OUT/'drive-1-3-5-air-plus12.wav',np.concatenate(reel),48000,subtype='FLOAT')
(HERE/'drive-validation.json').write_text(json.dumps(rows,indent=2)+'\n');print(rows)
