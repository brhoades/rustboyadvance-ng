use std::cell::RefCell;
use std::fmt;
use std::ops::Add;
use std::rc::Rc;

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};

use super::arm7tdmi::bus::Bus;
use super::arm7tdmi::Addr;
use super::gba::IoDevices;
use super::gpu::GpuState;
use super::{cartridge::Cartridge, ioregs::IoRegs};

const VIDEO_RAM_SIZE: usize = 128 * 1024;
const WORK_RAM_SIZE: usize = 256 * 1024;
const INTERNAL_RAM_SIZE: usize = 32 * 1024;
const PALETTE_RAM_SIZE: usize = 1 * 1024;
const OAM_SIZE: usize = 1 * 1024;

pub const BIOS_ADDR: u32 = 0x0000_0000;
pub const EWRAM_ADDR: u32 = 0x0200_0000;
pub const IWRAM_ADDR: u32 = 0x0300_0000;
pub const IOMEM_ADDR: u32 = 0x0400_0000;
pub const PALRAM_ADDR: u32 = 0x0500_0000;
pub const VRAM_ADDR: u32 = 0x0600_0000;
pub const OAM_ADDR: u32 = 0x0700_0000;
pub const GAMEPAK_WS0_ADDR: u32 = 0x0800_0000;
pub const GAMEPAK_WS1_ADDR: u32 = 0x0A00_0000;
pub const GAMEPAK_WS2_ADDR: u32 = 0x0C00_0000;

#[derive(Debug, Copy, Clone)]
pub enum MemoryAccessType {
    NonSeq,
    Seq,
}

impl fmt::Display for MemoryAccessType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                MemoryAccessType::NonSeq => "N",
                MemoryAccessType::Seq => "S",
            }
        )
    }
}

#[derive(Debug, PartialEq, Copy, Clone)]
pub enum MemoryAccessWidth {
    MemoryAccess8,
    MemoryAccess16,
    MemoryAccess32,
}

impl Add<MemoryAccessWidth> for MemoryAccessType {
    type Output = MemoryAccess;

    fn add(self, other: MemoryAccessWidth) -> Self::Output {
        MemoryAccess(self, other)
    }
}

#[derive(Debug, Copy, Clone)]
pub struct MemoryAccess(pub MemoryAccessType, pub MemoryAccessWidth);

impl fmt::Display for MemoryAccess {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}-Cycle ({:?})", self.0, self.1)
    }
}

#[derive(Debug)]
pub struct BoxedMemory {
    pub mem: Box<[u8]>,
    mask: u32,
}

impl BoxedMemory {
    pub fn new(boxed_slice: Box<[u8]>, mask: u32) -> BoxedMemory {
        BoxedMemory {
            mem: boxed_slice,
            mask: mask,
        }
    }
}

impl Bus for BoxedMemory {
    fn read_32(&self, addr: Addr) -> u32 {
        (&self.mem[(addr & self.mask) as usize..])
            .read_u32::<LittleEndian>()
            .unwrap()
    }

    fn read_16(&self, addr: Addr) -> u16 {
        (&self.mem[(addr & self.mask) as usize..])
            .read_u16::<LittleEndian>()
            .unwrap()
    }

    fn read_8(&self, addr: Addr) -> u8 {
        (&self.mem[(addr & self.mask) as usize..])[0]
    }

    fn write_32(&mut self, addr: Addr, value: u32) {
        (&mut self.mem[(addr & self.mask) as usize..])
            .write_u32::<LittleEndian>(value)
            .unwrap()
    }

    fn write_16(&mut self, addr: Addr, value: u16) {
        (&mut self.mem[(addr & self.mask) as usize..])
            .write_u16::<LittleEndian>(value)
            .unwrap()
    }

    fn write_8(&mut self, addr: Addr, value: u8) {
        (&mut self.mem[(addr & self.mask) as usize..])
            .write_u8(value)
            .unwrap()
    }
}

#[derive(Debug)]
struct DummyBus([u8; 4]);

impl Bus for DummyBus {
    fn read_32(&self, _addr: Addr) -> u32 {
        0
    }

    fn read_16(&self, _addr: Addr) -> u16 {
        0
    }

    fn read_8(&self, _addr: Addr) -> u8 {
        0
    }

    fn write_32(&mut self, _addr: Addr, _value: u32) {}

    fn write_16(&mut self, _addr: Addr, _value: u16) {}

    fn write_8(&mut self, _addr: Addr, _value: u8) {}
}

#[derive(Debug)]
pub struct SysBus {
    pub io: Rc<RefCell<IoDevices>>,

    bios: BoxedMemory,
    onboard_work_ram: BoxedMemory,
    internal_work_ram: BoxedMemory,
    /// Currently model the IOMem as regular buffer, later make it into something more sophisticated.
    pub ioregs: IoRegs,
    pub palette_ram: BoxedMemory,
    pub vram: BoxedMemory,
    pub oam: BoxedMemory,
    gamepak: Cartridge,
    dummy: DummyBus,
}

impl SysBus {
    pub fn new(
        io: Rc<RefCell<IoDevices>>,
        bios_rom: Vec<u8>,
        gamepak: Cartridge,
        ioregs: IoRegs,
    ) -> SysBus {
        SysBus {
            io: io,

            bios: BoxedMemory::new(bios_rom.into_boxed_slice(), 0xff_ffff),
            onboard_work_ram: BoxedMemory::new(
                vec![0; WORK_RAM_SIZE].into_boxed_slice(),
                (WORK_RAM_SIZE as u32) - 1,
            ),
            internal_work_ram: BoxedMemory::new(
                vec![0; INTERNAL_RAM_SIZE].into_boxed_slice(),
                0x7fff,
            ),
            ioregs: ioregs,
            palette_ram: BoxedMemory::new(
                vec![0; PALETTE_RAM_SIZE].into_boxed_slice(),
                (PALETTE_RAM_SIZE as u32) - 1,
            ),
            vram: BoxedMemory::new(
                vec![0; VIDEO_RAM_SIZE].into_boxed_slice(),
                (VIDEO_RAM_SIZE as u32) - 1,
            ),
            oam: BoxedMemory::new(vec![0; OAM_SIZE].into_boxed_slice(), (OAM_SIZE as u32) - 1),
            gamepak: gamepak,
            dummy: DummyBus([0; 4]),
        }
    }

    fn map(&self, addr: Addr) -> &Bus {
        match addr & 0xff000000 {
            BIOS_ADDR => &self.bios,
            EWRAM_ADDR => &self.onboard_work_ram,
            IWRAM_ADDR => &self.internal_work_ram,
            IOMEM_ADDR => &self.ioregs,
            PALRAM_ADDR => &self.palette_ram,
            VRAM_ADDR => &self.vram,
            OAM_ADDR => &self.oam,
            GAMEPAK_WS0_ADDR | GAMEPAK_WS1_ADDR | GAMEPAK_WS2_ADDR => &self.gamepak,
            _ => &self.dummy,
        }
    }

    /// TODO proc-macro for generating this function
    fn map_mut(&mut self, addr: Addr) -> &mut Bus {
        match addr & 0xff000000 {
            BIOS_ADDR => &mut self.bios,
            EWRAM_ADDR => &mut self.onboard_work_ram,
            IWRAM_ADDR => &mut self.internal_work_ram,
            IOMEM_ADDR => &mut self.ioregs,
            PALRAM_ADDR => &mut self.palette_ram,
            VRAM_ADDR => &mut self.vram,
            OAM_ADDR => &mut self.oam,
            GAMEPAK_WS0_ADDR | GAMEPAK_WS1_ADDR | GAMEPAK_WS2_ADDR => &mut self.gamepak,
            _ => &mut self.dummy,
        }
    }

    pub fn get_cycles(&self, addr: Addr, access: MemoryAccess) -> usize {
        let nonseq_cycles = [4, 3, 2, 8];
        let seq_cycles = [2, 1];

        let mut cycles = 0;

        // TODO handle EWRAM accesses
        match addr & 0xff000000 {
            EWRAM_ADDR => match access.1 {
                MemoryAccessWidth::MemoryAccess32 => cycles += 6,
                _ => cycles += 3,
            },
            OAM_ADDR | VRAM_ADDR | PALRAM_ADDR => {
                match access.1 {
                    MemoryAccessWidth::MemoryAccess32 => cycles += 2,
                    _ => cycles += 1,
                }
                if self.io.borrow().gpu.state == GpuState::HDraw {
                    cycles += 1;
                }
            }
            GAMEPAK_WS0_ADDR => match access.0 {
                MemoryAccessType::NonSeq => match access.1 {
                    MemoryAccessWidth::MemoryAccess32 => {
                        cycles += nonseq_cycles[self.ioregs.waitcnt.ws0_first_access() as usize];
                        cycles += seq_cycles[self.ioregs.waitcnt.ws0_second_access() as usize];
                    }
                    _ => {
                        cycles += nonseq_cycles[self.ioregs.waitcnt.ws0_first_access() as usize];
                    }
                },
                MemoryAccessType::Seq => {
                    cycles += seq_cycles[self.ioregs.waitcnt.ws0_second_access() as usize];
                    if access.1 == MemoryAccessWidth::MemoryAccess32 {
                        cycles += seq_cycles[self.ioregs.waitcnt.ws0_second_access() as usize];
                    }
                }
            },
            GAMEPAK_WS1_ADDR | GAMEPAK_WS2_ADDR => {
                panic!("unimplemented - need to refactor code with a nice macro :(")
            }
            _ => {}
        }

        cycles
    }
}

impl Bus for SysBus {
    fn read_32(&self, addr: Addr) -> u32 {
        self.map(addr).read_32(addr & 0xff_ffff)
    }

    fn read_16(&self, addr: Addr) -> u16 {
        self.map(addr).read_16(addr & 0xff_ffff)
    }

    fn read_8(&self, addr: Addr) -> u8 {
        self.map(addr).read_8(addr & 0xff_ffff)
    }

    fn write_32(&mut self, addr: Addr, value: u32) {
        self.map_mut(addr).write_32(addr & 0xff_ffff, value)
    }

    fn write_16(&mut self, addr: Addr, value: u16) {
        self.map_mut(addr).write_16(addr & 0xff_ffff, value)
    }

    fn write_8(&mut self, addr: Addr, value: u8) {
        self.map_mut(addr).write_8(addr & 0xff_ffff, value)
    }
}
