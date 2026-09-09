#!/usr/bin/env python3
"""Plot measured level compression; no fitted model or loudness normalization."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt


def plot(report, output):
    files=json.loads(report.read_text())['files']
    levels=[-24,-12,-6,-3,0,3,6]
    fig,axes=plt.subplots(1,2,figsize=(11,4.5),sharey=True,layout='constrained')
    for ax,res in zip(axes,['0','0.8']):
        for typ,color in [(1,'#3466b0'),(2,'#c76b23')]:
            base=files[f't{typ}-sine-r{res}--24']['notes'][1]['fundamental_dbfs']
            values=[]
            for level in levels:
                tag=str(level) if level<0 else 'p'+str(level)
                n=files[f't{typ}-sine-r{res}-{tag}']['notes'][1]
                values.append(n['fundamental_dbfs']-base-(level+24))
            ax.plot(levels,values,'o-',label=f'Type {typ}',color=color,lw=2)
        ax.axhline(0,color='#555555',lw=.7)
        ax.set_title(f'Resonance {res}')
        ax.set_xlabel('Oscillator mixer level (dB)')
        ax.set_xticks(levels)
        ax.grid(alpha=.2)
        ax.legend(frameon=False)
    axes[0].set_ylabel('Fundamental gain change vs quiet input (dB)')
    fig.suptitle('Drift: turning up the oscillator changes the filter response',fontsize=14)
    fig.supxlabel('Live 12.4.5 · 48 kHz · Sine, MIDI 48 · cutoff 1 kHz · −24 dB input is the reference',fontsize=9)
    fig.savefig(output,dpi=180)
    plt.close(fig)


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('report',type=Path)
    p.add_argument('output',type=Path)
    a=p.parse_args()
    plot(a.report,a.output)
