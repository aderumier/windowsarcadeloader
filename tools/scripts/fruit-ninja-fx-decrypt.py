#!/usr/bin/env python3
"""Decrypt this exact Fruit Ninja FX Matrix-envelope dump; Python standard library only.

The recovered XTEA key and entry-point transform are documented in FINDINGS.md.
The encrypted input is always preserved. Other builds are rejected by SHA-256.
"""
import argparse
import hashlib
import json
import struct
from pathlib import Path
MASK = 0xffffffff
SOURCE_SHA256 = '6c8254348af42e7ccc24719b0d1eecc0856a6d7207d391be4144c597c93fc460'
KEY = bytes.fromhex('d16571b43c9720a978297466ffe3a73a')
def decrypt(data,key):
 out=bytearray(data);key=struct.unpack('<4I',key)
 for off in range(0,len(out)//8*8,8):
  a,b=struct.unpack_from('<II',out,off);s=0xc6ef3720
  for _ in range(32):
   b=(b-((((a>>5)^(a<<4))+a)^((s+key[(s>>11)&3])&MASK)))&MASK;s=(s-0x9e3779b9)&MASK
   a=(a-((((b>>5)^(b<<4))+b)^((s+key[s&3])&MASK)))&MASK
  struct.pack_into('<II',out,off,a,b)
 return out
def depack(data,limit=8192):
 pos=0;bits=0;left=0;out=bytearray();last=None;lwm=2
 def byte():
  nonlocal pos
  if pos>=len(data):raise EOFError()
  v=data[pos];pos+=1;return v
 def bit():
  nonlocal bits,left
  if not left:bits=byte();left=8
  left-=1;return (bits>>left)&1
 def gamma():
  n=1
  for _ in range(30):
   n=n*2+bit()
   if not bit():return n
  raise ValueError('gamma overflow')
 def copy(off,n):
  if off is None or off<=0 or off>len(out) or n>limit:raise ValueError('invalid backreference')
  for _ in range(n):
   out.append(out[-off])
   if len(out)>=limit:raise EOFError()
 try:
  out.append(byte())
  while len(out)<limit:
   if not bit():out.append(byte());lwm=2
   elif not bit():
    off=gamma()-lwm;lwm=1
    if off==0:copy(last,gamma())
    else:
     off=((off-1)<<8)|byte();n=gamma();n+=(off>=32000)+(off>=1280)+2*(off<128);last=off;copy(off,n)
   elif not bit():
    v=byte();off=v>>1
    if not off:return 'complete',pos,out
    last=off;copy(off,2+(v&1));lwm=1
   else:
    off=0
    for _ in range(4):off=off*2+bit()
    if off:copy(off,1)
    else:out.append(0)
    lwm=2
 except EOFError:return 'prefix',pos,out
 except (ValueError,IndexError):return 'invalid',pos,out
 return 'prefix',pos,out

def main():
 parser=argparse.ArgumentParser(description=__doc__)
 parser.add_argument('input',type=Path)
 parser.add_argument('output',type=Path)
 args=parser.parse_args()
 if args.input.resolve()==args.output.resolve():parser.error('input and output must differ')
 raw=args.input.read_bytes()
 if hashlib.sha256(raw).hexdigest()!=SOURCE_SHA256:parser.error('unsupported input SHA-256')
 pe=struct.unpack_from('<I',raw,0x3c)[0];opt=pe+24
 count=struct.unpack_from('<H',raw,pe+6)[0]
 section_table=opt+struct.unpack_from('<H',raw,pe+20)[0]
 sections=[]
 for i in range(count):
  off=section_table+40*i
  vsize,rva,n,ptr=struct.unpack_from('<4I',raw,off+8)
  sections.append((off,vsize,rva,n,ptr))
 # The original timestamp is the envelope's pair of compression/encryption flags.
 flags=struct.unpack_from('<I',raw,pe+8)[0]
 image_end=sections[-1][2]
 image=bytearray(image_end)
 headers=struct.unpack_from('<I',raw,opt+60)[0]
 image[:headers]=raw[:headers]
 rows=[]
 for i,(off,vsize,rva,n,ptr) in enumerate(sections[:-1]):
  data=raw[ptr:ptr+n]
  trim=len(data.rstrip(b'\0'))
  if flags&(1<<(2*i+1)):data=decrypt(data[:trim],KEY)+data[trim:]
  if flags&(1<<(2*i)):
   state,consumed,data=depack(data,limit=vsize+1)
   if state!='complete' or len(data)>vsize:raise ValueError(f'section {i}: invalid compressed stream')
  if len(data)>vsize:raise ValueError('section exceeds virtual capacity')
  image[rva:rva+len(data)]=data
  rows.append({'rva':hex(rva),'decoded_bytes':len(data),'sha256':hashlib.sha256(image[rva:rva+vsize]).hexdigest()})
  name=[b'.text',b'.rdata',b'.data',b'.rsrc'][i]
  image[off:off+8]=name.ljust(8,b'\0')
  struct.pack_into('<II',image,off+16,vsize,rva)
  struct.pack_into('<I',image,off+36,[0x60000020,0x40000040,0xc0000040,0x40000040][i])
 # Reconstruct the original entry from the exception handler and key checksum.
 magic=0xc3c3c3c3^0x11091973
 intermediate=((0xff596e36+magic+1)&MASK)^magic
 key_checksum=0
 for i,c in enumerate(KEY):key_checksum=((key_checksum+c*i)*2)&MASK
 entry=intermediate^key_checksum
 if entry!=0x651db:raise ValueError('entry-point verification failed')
 imports=0x14e17c
 descriptors=0
 while any(image[imports+20*descriptors:imports+20*(descriptors+1)]):
  descriptors+=1
  if descriptors>256:raise ValueError('invalid import descriptor list')
 struct.pack_into('<H',image,pe+6,count-1)
 struct.pack_into('<I',image,pe+8,0)
 struct.pack_into('<I',image,opt+16,entry)
 struct.pack_into('<I',image,opt+56,image_end)
 struct.pack_into('<I',image,opt+64,0)
 struct.pack_into('<II',image,opt+96+8,imports,(descriptors+1)*20)
 struct.pack_into('<II',image,opt+96+12*8,0,0)
 image[section_table+40*(count-1):section_table+40*count]=b'\0'*40
 args.output.write_bytes(image)
 print(json.dumps({'output':str(args.output),'sha256':hashlib.sha256(image).hexdigest(),'entry_rva':hex(entry),'import_dlls':descriptors,'sections':rows},indent=2))

if __name__=='__main__':main()
