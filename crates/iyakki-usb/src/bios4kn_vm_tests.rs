//! F-INST-29: the 4096-byte-sector boot code (`dist/boot4k/boot4k.bin`) run on a small real-mode interpreter against a fake BIOS and a
//! byte image of a disk. Only the instructions the code uses are known; any other opcode stops the run with its address, so a change to
//! the source that adds one must also teach this file about it.

use crate::bios4kn::{self, BUILTIN_CODE, SECTOR};

#[derive(Debug, PartialEq, Eq)]
enum Stop {
    /// `ljmp` to seg:off after the code finished its work.
    Jumped(u16, u16),
    /// `hlt` after the error message.
    Halted,
}

struct Vm<'a> {
    mem: Vec<u8>,
    r: [u16; 8], // ax cx dx bx sp bp si di
    ip: u16,
    cs: u16,
    zf: bool,
    cf: bool,
    disk: &'a [u8],
    sector_size: usize,
    printed: String,
    /// (lba, count, linear address) of each BIOS read
    reads: Vec<(u64, u16, usize)>,
    edd: bool,
}

const AX: usize = 0;
const CX: usize = 1;
const DX: usize = 2;
const BX: usize = 3;
const SI: usize = 6;

impl<'a> Vm<'a> {
    fn new(sector0: &[u8], disk: &'a [u8], sector_size: usize) -> Vm<'a> {
        let mut mem = vec![0u8; 1 << 20];
        mem[0x7C00..0x7C00 + sector0.len()].copy_from_slice(sector0);
        // the BIOS enters at 0000:7C00 with the drive number in DL
        let mut r = [0u16; 8];
        r[DX] = 0x0080;
        Vm { mem, r, ip: 0x7C00, cs: 0, zf: false, cf: false, disk, sector_size, printed: String::new(), reads: vec![], edd: true }
    }
    fn lin(seg: u16, off: u16) -> usize {
        (seg as usize) * 16 + off as usize
    }
    fn b(&self, a: usize) -> u8 {
        self.mem[a]
    }
    fn w(&self, a: usize) -> u16 {
        u16::from_le_bytes([self.mem[a], self.mem[a + 1]])
    }
    fn put_w(&mut self, a: usize, v: u16) {
        self.mem[a..a + 2].copy_from_slice(&v.to_le_bytes());
    }
    fn fetch8(&mut self) -> u8 {
        let v = self.b(Self::lin(self.cs, self.ip));
        self.ip = self.ip.wrapping_add(1);
        v
    }
    fn fetch16(&mut self) -> u16 {
        let lo = self.fetch8() as u16;
        lo | (self.fetch8() as u16) << 8
    }
    fn set8(&mut self, reg: usize, v: u8) {
        // registers 0..3 low (al cl dl bl), 4..7 high (ah ch dh bh)
        let r = reg & 3;
        if reg < 4 {
            self.r[r] = (self.r[r] & 0xFF00) | v as u16;
        } else {
            self.r[r] = (self.r[r] & 0x00FF) | (v as u16) << 8;
        }
    }
    fn get8(&self, reg: usize) -> u8 {
        let r = reg & 3;
        if reg < 4 {
            self.r[r] as u8
        } else {
            (self.r[r] >> 8) as u8
        }
    }
    fn push(&mut self, v: u16) {
        self.r[4] = self.r[4].wrapping_sub(2);
        self.put_w(self.r[4] as usize, v);
    }
    fn pop(&mut self) -> u16 {
        let v = self.w(self.r[4] as usize);
        self.r[4] = self.r[4].wrapping_add(2);
        v
    }
    fn jcc(&mut self, cond: bool) {
        let d = self.fetch8() as i8;
        if cond {
            self.ip = self.ip.wrapping_add(d as u16);
        }
    }
    fn int13(&mut self) {
        let ah = self.get8(4);
        self.cf = false;
        match ah {
            0x41 => {
                if self.edd && self.r[BX] == 0x55AA {
                    self.r[BX] = 0xAA55;
                    self.r[CX] = 1;
                } else {
                    self.cf = true;
                }
            }
            0x42 => {
                let p = self.r[SI] as usize; // DS is 0 here
                if self.b(p) != 0x10 || self.get8(2) != 0x80 {
                    self.cf = true;
                    return;
                }
                let count = self.w(p + 2);
                let (off, seg) = (self.w(p + 4), self.w(p + 6));
                let lba = u64::from_le_bytes(self.mem[p + 8..p + 16].try_into().unwrap());
                assert!(count <= 0x7F, "the code asked for more than the EDD limit");
                // a transfer must not cross a 64 KiB boundary of the linear address (many BIOSes refuse it)
                let at = Self::lin(seg, off);
                assert!(at / 0x10000 == (at + count as usize * self.sector_size - 1) / 0x10000, "a read crosses a 64 KiB boundary at {at:#x}");
                self.reads.push((lba, count, at));
                let from = lba as usize * self.sector_size;
                let n = count as usize * self.sector_size;
                if from + n > self.disk.len() {
                    self.cf = true;
                    return;
                }
                self.mem[at..at + n].copy_from_slice(&self.disk[from..from + n]);
            }
            other => panic!("int 13h function {other:#x} is not faked"),
        }
    }
    /// modrm for the one form used: mod=00 rm=110 (a 16-bit absolute address); returns (reg field, address) or the register for mod=11.
    fn modrm_abs(&mut self, m: u8) -> (usize, usize) {
        assert_eq!(m & 0xC7, 0x06, "unsupported modrm {m:#x} at {:#x}", self.ip);
        let a = self.fetch16() as usize;
        (((m >> 3) & 7) as usize, a)
    }
    fn run(&mut self, max: usize) -> Stop {
        for _ in 0..max {
            let at = self.ip;
            let op = self.fetch8();
            match op {
                0xFA | 0xFB => {} // cli sti
                0x31 => {
                    let m = self.fetch8();
                    assert_eq!(m, 0xC0, "xor form at {at:#x}");
                    self.r[AX] = 0;
                    self.zf = true;
                    self.cf = false;
                }
                0x8E => {
                    let m = self.fetch8();
                    assert!(matches!(m, 0xD8 | 0xC0 | 0xD0), "mov sreg at {at:#x}");
                    assert_eq!(self.r[AX], 0, "segment registers are only loaded with zero here");
                }
                0xBC => self.r[4] = self.fetch16(),
                0xBB => self.r[BX] = self.fetch16(),
                0xBE => self.r[SI] = self.fetch16(),
                0xB8 => self.r[AX] = self.fetch16(),
                0xB4 => {
                    let v = self.fetch8();
                    self.set8(4, v)
                }
                0xEA => {
                    let (o, s) = (self.fetch16(), self.fetch16());
                    if s == 0 && Self::lin(s, o) >= 0x9000 {
                        return Stop::Jumped(s, o);
                    }
                    self.cs = s;
                    self.ip = o;
                }
                0x88 | 0x8A => {
                    let m = self.fetch8();
                    if m & 0xC0 == 0xC0 {
                        // mov %al,%ah style register move (88 /r: rm <- reg)
                        let (reg, rm) = (((m >> 3) & 7) as usize, (m & 7) as usize);
                        if op == 0x88 {
                            let v = self.get8(reg);
                            self.set8(rm, v)
                        } else {
                            let v = self.get8(rm);
                            self.set8(reg, v)
                        }
                    } else {
                        let (reg, a) = self.modrm_abs(m);
                        if op == 0x88 {
                            self.mem[a] = self.get8(reg)
                        } else {
                            let v = self.mem[a];
                            self.set8(reg, v)
                        }
                    }
                }
                0x30 => {
                    let m = self.fetch8();
                    assert_eq!(m, 0xC0, "xor al,al only");
                    self.set8(0, 0);
                    self.zf = true;
                }
                0xA1 => {
                    let a = self.fetch16() as usize;
                    self.r[AX] = self.w(a)
                }
                0xA3 => {
                    let a = self.fetch16() as usize;
                    self.put_w(a, self.r[AX])
                }
                0xC7 => {
                    let m = self.fetch8();
                    let (reg, a) = self.modrm_abs(m);
                    assert_eq!(reg, 0);
                    let v = self.fetch16();
                    self.put_w(a, v)
                }
                0x01 | 0x29 => {
                    let m = self.fetch8();
                    let (_, a) = self.modrm_abs(m);
                    let (x, y) = (self.w(a), self.r[AX]);
                    let (v, c) = if op == 0x01 { x.overflowing_add(y) } else { x.overflowing_sub(y) };
                    self.put_w(a, v);
                    self.cf = c;
                    self.zf = v == 0;
                }
                0xB9 => self.r[CX] = self.fetch16(),
                0x8B => {
                    let m = self.fetch8();
                    let (reg, a) = self.modrm_abs(m);
                    self.r[reg] = self.w(a);
                }
                0x39 => {
                    // cmp r/m16,r16 with both registers: flags of rm - reg
                    let m = self.fetch8();
                    assert_eq!(m >> 6, 3);
                    let (x, y) = (self.r[(m & 7) as usize], self.r[((m >> 3) & 7) as usize]);
                    self.cf = x < y;
                    self.zf = x == y;
                }
                0x89 => {
                    let m = self.fetch8();
                    assert_eq!(m >> 6, 3);
                    self.r[(m & 7) as usize] = self.r[((m >> 3) & 7) as usize];
                }
                0xF7 => {
                    let m = self.fetch8();
                    assert_eq!(m & 0xF8, 0xD8, "neg only");
                    let r = (m & 7) as usize;
                    self.cf = self.r[r] != 0;
                    self.r[r] = self.r[r].wrapping_neg();
                    self.zf = self.r[r] == 0;
                }
                0x83 => {
                    let m = self.fetch8();
                    if m >> 6 == 3 {
                        let r = (m & 7) as usize;
                        let v = self.fetch8() as i8 as u16;
                        match m >> 3 & 7 {
                            0 => {
                                let (x, c) = self.r[r].overflowing_add(v);
                                self.r[r] = x;
                                self.cf = c;
                                self.zf = x == 0;
                            }
                            4 => {
                                self.r[r] &= v;
                                self.zf = self.r[r] == 0;
                                self.cf = false;
                            }
                            7 => {
                                self.cf = self.r[r] < v;
                                self.zf = self.r[r] == v;
                            }
                            n => panic!("83 /{n} at {at:#x}"),
                        }
                    } else {
                        let (reg, a) = self.modrm_abs(m);
                        assert_eq!(reg, 2, "adc only at {at:#x}");
                        let v = self.fetch8() as i8 as u16;
                        let (s1, c1) = self.w(a).overflowing_add(v);
                        let (s2, c2) = s1.overflowing_add(self.cf as u16);
                        self.put_w(a, s2);
                        self.cf = c1 | c2;
                        self.zf = s2 == 0;
                    }
                }
                0x81 => {
                    let m = self.fetch8();
                    assert_eq!(m, 0xFB, "cmp bx,imm only");
                    let v = self.fetch16();
                    self.zf = self.r[BX] == v;
                    self.cf = self.r[BX] < v;
                }
                0xF6 => {
                    let m = self.fetch8();
                    assert_eq!(m, 0xC1, "test cl,imm only");
                    let v = self.fetch8();
                    self.zf = self.get8(1) & v == 0;
                    self.cf = false;
                }
                0x85 => {
                    assert_eq!(self.fetch8(), 0xC0);
                    self.zf = self.r[AX] == 0;
                    self.cf = false;
                }
                0x84 => {
                    assert_eq!(self.fetch8(), 0xC0);
                    self.zf = self.get8(0) == 0;
                    self.cf = false;
                }
                0xAC => {
                    let v = self.mem[self.r[SI] as usize];
                    self.set8(0, v);
                    self.r[SI] = self.r[SI].wrapping_add(1);
                }
                0xCD => match self.fetch8() {
                    0x13 => self.int13(),
                    0x10 => {
                        assert_eq!(self.get8(4), 0x0E);
                        self.printed.push(self.get8(0) as char);
                    }
                    n => panic!("int {n:#x} is not faked"),
                },
                0xE8 => {
                    let d = self.fetch16();
                    self.push(self.ip);
                    self.ip = self.ip.wrapping_add(d);
                }
                0xC3 => self.ip = self.pop(),
                0xEB => self.jcc(true),
                0x72 => self.jcc(self.cf),
                0x73 => self.jcc(!self.cf),
                0x74 => self.jcc(self.zf),
                0x75 => self.jcc(!self.zf),
                0x76 => self.jcc(self.cf || self.zf),
                0xF4 => return Stop::Halted,
                0x0F => {
                    let c = self.fetch8();
                    let d = self.fetch16() as i16;
                    let cond = match c {
                        0x82 => self.cf,
                        0x83 => !self.cf,
                        0x84 => self.zf,
                        0x86 => self.cf || self.zf,
                        0x85 => !self.zf,
                        _ => panic!("0f {c:#x} at {at:#x}"),
                    };
                    if cond {
                        self.ip = self.ip.wrapping_add(d as u16);
                    }
                }
                other => panic!("opcode {other:#x} at {at:#x} is not known to the interpreter"),
            }
        }
        panic!("the boot code did not finish in {max} steps");
    }
}

/// A 4096-byte-sector disk image: the installer's sector 0 and core image for a 512-byte-sector core of `rest512` sectors.
fn disk(core_lba: u64, rest512: u16) -> (Vec<u8>, Vec<u8>) {
    let mut core: Vec<u8> = (0..512 * (1 + rest512 as usize)).map(|i| (i * 7 % 253) as u8 | 1).collect();
    core[500..508].copy_from_slice(&2u64.to_le_bytes());
    core[508..510].copy_from_slice(&rest512.to_le_bytes());
    core[510..512].copy_from_slice(&0x820u16.to_le_bytes());
    core[488..500].fill(0);
    let mut old = vec![0u8; SECTOR];
    old[510] = 0x55;
    old[511] = 0xAA;
    let l = bios4kn::build(&old, BUILTIN_CODE, &core, core_lba, 200).unwrap();
    let mut d = vec![0u8; SECTOR * 400];
    d[..SECTOR].copy_from_slice(&l.sector0);
    d[core_lba as usize * SECTOR..core_lba as usize * SECTOR + l.core.len()].copy_from_slice(&l.core);
    (d, l.core)
}

#[test]
fn the_built_in_code_has_the_shape_the_installer_needs() {
    assert!(bios4kn::check_boot_code(BUILTIN_CODE).is_ok());
    assert_eq!(BUILTIN_CODE[0], 0xEB, "a short jump over the packet");
    assert_eq!(&BUILTIN_CODE[0x5C..0x64], &[0; 8], "the core address field is empty until the installer fills it");
    assert_eq!(&BUILTIN_CODE[0x54..0x5C], &[0x10, 0, 1, 0, 0, 0, 0, 8], "the disk address packet in front of the field");
    assert_eq!(bios4kn::load_code(bios4kn::BUILTIN_NAME).unwrap(), BUILTIN_CODE.to_vec());
    assert!(bios4kn::load_code("/no/such/boot/code").is_err());
}

#[test]
fn it_reads_the_core_in_4096_byte_units_with_the_segment_stepping_and_jumps_behind_the_first_sector() {
    // 40 sectors of 512 bytes behind the first one = 5 sectors of 4096 bytes
    let (d, core) = disk(300, 40);
    let s0 = d[..SECTOR].to_vec();
    let mut vm = Vm::new(&s0, &d, SECTOR);
    assert_eq!(vm.run(10_000), Stop::Jumped(0, 0x9000));
    // first the core's first sector to 0x8000, then the rest (5 sectors) at the segment from the block list (0x900)
    assert_eq!(vm.reads, vec![(300, 1, 0x8000), (301, 5, 0x9000)]);
    assert_eq!(&vm.mem[0x8000..0x8000 + core.len()], &core[..], "the core sits in memory as the installer laid it out");
    assert_eq!(vm.r[DX] as u8, 0x80, "the drive number is passed on");
}

#[test]
fn a_long_core_is_read_eight_sectors_at_a_time_and_the_segment_steps_by_0x100_per_sector() {
    // 130 sectors of 512 bytes = 17 sectors of 4096 bytes, loaded from 0x9000: 7 up to the 64 KiB boundary, then 8, then 2
    let (d, core) = disk(300, 130);
    let s0 = d[..SECTOR].to_vec();
    let mut vm = Vm::new(&s0, &d, SECTOR);
    assert_eq!(vm.run(10_000), Stop::Jumped(0, 0x9000));
    assert_eq!(vm.reads, vec![(300, 1, 0x8000), (301, 7, 0x9000), (308, 8, 0x10000), (316, 2, 0x18000)]);
    assert_eq!(&vm.mem[0x8000..0x8000 + core.len()], &core[..]);
}

#[test]
fn the_start_sector_carries_through_all_eight_bytes() {
    // a core whose block list starts just below 2^16 sectors: the add must carry into the next word
    let (mut d, _) = disk(300, 130);
    // move the block list start to 0xFFFC (4096-byte units, inside the fake disk we only check the packet, so read errors are fine)
    let at = 300 * SECTOR + 500;
    d[at..at + 8].copy_from_slice(&0xFFFCu64.to_le_bytes());
    let s0 = d[..SECTOR].to_vec();
    let big = vec![0u8; SECTOR * 0x10040];
    let mut disk = big;
    disk[..SECTOR].copy_from_slice(&d[..SECTOR]);
    disk[300 * SECTOR..300 * SECTOR + 3 * SECTOR + 17 * SECTOR].copy_from_slice(&d[300 * SECTOR..300 * SECTOR + 20 * SECTOR]);
    let mut vm = Vm::new(&s0, &disk, SECTOR);
    assert_eq!(vm.run(10_000), Stop::Jumped(0, 0x9000));
    assert_eq!(vm.reads.iter().map(|r| r.0).collect::<Vec<_>>(), vec![300, 0xFFFC, 0x10003, 0x1000B]);
}

#[test]
fn without_edd_or_with_an_unreadable_disk_it_says_so_and_halts() {
    let (d, _) = disk(300, 40);
    let s0 = d[..SECTOR].to_vec();
    let mut vm = Vm::new(&s0, &d, SECTOR);
    vm.edd = false;
    assert_eq!(vm.run(10_000), Stop::Halted);
    assert_eq!(vm.printed, "IYAKKI 4K boot error\r\n");
    assert!(vm.reads.is_empty());
    let mut vm = Vm::new(&s0, &d[..SECTOR * 10], SECTOR);
    assert_eq!(vm.run(10_000), Stop::Halted, "the core is beyond the end of the disk");
    assert!(vm.printed.contains("boot error"));
}
