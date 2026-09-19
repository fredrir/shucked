#!/usr/bin/env python3
"""Check PE architecture and DLL availability; target loader execution remains required."""
import argparse
from pathlib import Path
import struct

SYSTEM_DLLS = set('advapi32 avrt bcrypt cabinet cfgmgr32 comctl32 comdlg32 crypt32 cryptbase dbghelp dnsapi dwmapi gdi32 glu32 imm32 iphlpapi kernel32 mpr msimg32 msvcrt netapi32 ntdll ole32 oleacc oleaut32 opengl32 powrprof propsys psapi rpcrt4 secur32 setupapi shell32 shlwapi user32 userenv usp10 uxtheme version winhttp wininet winmm winspool wintrust wldap32 ws2_32 wtsapi32'.split())


def imports(data):
    def u16(offset): return struct.unpack_from('<H', data, offset)[0]
    def u32(offset): return struct.unpack_from('<I', data, offset)[0]
    pe=u32(0x3c)
    if data[pe:pe+4] != b'PE\0\0': raise ValueError('invalid PE signature')
    machine=u16(pe+4)
    if machine != 0x8664: raise ValueError(f'expected x64 PE, found {machine:x}')
    sections=u16(pe+6)
    optional=pe+24
    magic=u16(optional)
    if magic != 0x20b: raise ValueError('expected PE32+')
    base=struct.unpack_from('<Q', data, optional+24)[0]
    directories=optional+112
    section_table=optional+u16(pe+20)
    def offset(rva):
        for index in range(sections):
            section=section_table+40*index
            size,virtual,raw_size,raw=struct.unpack_from('<IIII', data, section+8)
            if virtual <= rva < virtual+max(size,raw_size):
                result=raw+rva-virtual
                if result < len(data): return result
        raise ValueError(f'PE RVA outside file: {rva:x}')
    def name(rva):
        start=offset(rva)
        end=data.find(b'\0',start,min(start+512,len(data)))
        if end<0: raise ValueError('unterminated DLL name')
        result=data[start:end].decode('ascii').lower()
        if '/' in result or '\\' in result: raise ValueError('DLL import contains path')
        return result
    result=set()
    for directory,width,name_offset in ((1,20,12),(13,32,4)):
        if u32(optional+108)<=directory: continue
        rva,size=struct.unpack_from('<II',data,directories+8*directory)
        if not rva: continue
        start=offset(rva)
        for position in range(start,min(start+size,len(data)),width):
            record=data[position:position+width]
            if len(record)!=width: raise ValueError('truncated PE import')
            if not any(record): break
            name_rva=u32(position+name_offset)
            if directory==13 and not u32(position)&1: name_rva-=base
            result.add(name(name_rva))
        else: raise ValueError('unterminated PE import directory')
    return result


def verify(root):
    files=list(root.rglob('*'))
    available={}
    for path in files:
        if path.is_file() and path.suffix.lower()=='.dll': available.setdefault(path.name.lower(),set()).add(path.parent)
    checked=0
    for path in files:
        if not path.is_file() or 'sources' in path.relative_to(root).parts: continue
        with path.open('rb') as source:
            if source.read(2)!=b'MZ': continue
            source.seek(0)
            try: required=imports(source.read())
            except (ValueError,struct.error) as error: raise ValueError(f'{path}: {error}') from error
        for dependency in required:
            candidates=available.get(dependency,set())
            if candidates or dependency.removesuffix('.dll') in SYSTEM_DLLS or dependency.startswith(('api-ms-win-', 'ext-ms-win-')): continue
            raise ValueError(f'unbundled DLL: {path}: {dependency}')
        checked+=1
    if checked==0: raise ValueError('no PE programs found')
    return checked


if __name__=='__main__':
    parser=argparse.ArgumentParser()
    parser.add_argument('root',type=Path)
    args=parser.parse_args()
    print(f'Checked {verify(args.root)} x64 PE files; execution remains untested.')
