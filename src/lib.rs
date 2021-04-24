use std::collections::VecDeque;

/*
- interrupts
- returning from interrupts

- system level, unlimited access to memory
- user level, limited access to memory

*/

const READ:  u8 = 0b100;
const WRITE: u8 = 0b010;
const EXEC:  u8 = 0b001;

#[derive(Debug)]
pub enum InvalidMemoryAccess {
    UsedFreePage,
    InvalidPermissions(u8, u8),
    UnprivilegedOpcode
}

impl std::fmt::Display for InvalidMemoryAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "Invalid memory access")
    }
}

impl std::error::Error for InvalidMemoryAccess {}

pub trait Address {
    fn read(&mut self, addr: u32) -> u8;

    fn write(&mut self, addr: u32, data: u8);
}

const SIMPLE_ADDRESS_SIZE: usize = 0x1000000;

pub struct SimpleAddress {
    memory: Vec<u8>,
}

impl Default for SimpleAddress {
    fn default() -> SimpleAddress {
        SimpleAddress {
            memory: vec![0; SIMPLE_ADDRESS_SIZE],
        }
    }
}

impl Address for SimpleAddress {
    fn read(&mut self, addr: u32) -> u8 {
        if addr < 0x1000000 {
            self.memory[addr as usize]
        } else {
            0
        }
    }

    fn write(&mut self, addr: u32, data: u8) {
        if addr < 0x1000000 {
            self.memory[addr as usize] = data;
        }
    }
}

pub struct Cpu<T>
where
    T: Address,
{
    // Registers
    // General purpose integer registers
    // Program counter is x13
    // Stack base pointer is x14
    // Stack pointer is x15
    xs: [u32; 16],

    // General purpose floating point registers
    fs: [f32; 16],

    // Flags
    //                      MRFAN PCVZQLLL
    // 10987654 32109876 54321098 76543210
    // 33222222 22221111 111111
    // LLL      - Last interrupt
    // Q        - Interrupt enable
    // Z        - Zero
    // V        - oVerflow
    // C        - Carry
    // P        - Parity
    // N        - Negative
    // A        - nAn
    // F        - inFinite
    // R        - user Ring (if enabled, certain features will be locked down until an interrupt
    //            occurs)
    // M        - Memory map enable
    flags: u32,

    // Bits that are marked as 0 disable those interrupts from being added to the queue and being
    // handled
    interrupt_mask: u8,

    // Memory map register
    memmap: u32,

    // System ring stack pointer (saved from x15 when switching to the user ring)
    system_sp: u32,

    // Queue of previously requested interrupts
    interrupt_queue: VecDeque<u32>,

    addressing: T,
}

// Flags
static F_INTERRUPT_ENABLE: u32 = 3;
static F_ZERO: u32 = 4;
static F_OVERFLOW: u32 = 5;
static F_CARRY: u32 = 6;
static F_PARITY: u32 = 7;
static F_NEGATIVE: u32 = 8;
static F_NAN: u32 = 9;
static F_INFINITE: u32 = 10;
static F_USER_RING: u32 = 11;
static F_MEMMAP_ENABLE: u32 = 12;

// Registers
static R_INT: usize = 12;
static R_PC: usize = 13;
static R_BASE: usize = 14;
static R_SP: usize = 15;

macro_rules! clear_flags {
    ($self: ident, $($flags: ident),*) => {
        $self.flags &= !($((1 << $flags))|*);
    }
}

impl<T> Cpu<T>
where
    T: Address,
{
    pub fn new(t: T) -> Cpu<T> {
        Cpu {
            xs: [0; 16],
            fs: [0.0; 16],
            flags: 0,
            interrupt_mask: 0xff,
            memmap: 0,
            system_sp: 0,
            interrupt_queue: VecDeque::new(),
            addressing: t,
        }
    }

    fn check_memory(&mut self, addr: u32, permissions: u8) -> Result<u32, InvalidMemoryAccess> {
        if self.flags & (1 << F_MEMMAP_ENABLE) != 0 {
            let table_addr = self.memmap;
            let table_addr = self.addressing.read(table_addr + (addr >> 24)) as u32
                | (self.addressing.read(table_addr + (addr >> 24) + 1) as u32) << 8
                | (self.addressing.read(table_addr + (addr >> 24) + 2) as u32) << 16
                | (self.addressing.read(table_addr + (addr >> 24) + 3) as u32) << 24;

            if table_addr == 0 {
                return Err(InvalidMemoryAccess::UsedFreePage);
            }

            let addr = (self.addressing.read(table_addr + (addr >> 16 & 0xff)) as u32
                | (self.addressing.read(table_addr + (addr >> 16 & 0xff) + 1) as u32) << 8
                | (self.addressing.read(table_addr + (addr >> 16 & 0xff) + 2) as u32) << 16
                | (self.addressing.read(table_addr + (addr >> 16 & 0xff) + 3) as u32) << 24)
                + (addr & 0xffff);
            let (p, addr) = (((addr & 0xf0000000) >> 28) as u8, addr & 0x0fffffff);

            if p & 0x08 == 0 {
                Err(InvalidMemoryAccess::UsedFreePage)
            } else if p & permissions != permissions {
                Err(InvalidMemoryAccess::InvalidPermissions(p, permissions))
            } else {
                Ok(addr)
            }
        } else {
            Ok(addr)
        }
    }

    fn set_flag(&mut self, flag: u32, val: bool) {
        self.flags |= (val as u32) << flag;
    }

    fn get_flag(&self, flag: u32) -> bool {
        self.flags & (1 << flag) != 0
    }

    fn set_carry(&mut self, val: bool) {
        clear_flags!(self, F_CARRY);
        self.set_flag(F_CARRY, val);
    }

    fn set_user_ring(&mut self, val: bool) -> Result<(), InvalidMemoryAccess> {
        if !self.get_flag(F_USER_RING) {
            clear_flags!(self, F_USER_RING);
            self.set_flag(F_USER_RING, val);
            Ok(())
        } else {
            Err(InvalidMemoryAccess::UnprivilegedOpcode)
        }
    }

    fn set_memmap_enable(&mut self, val: bool) -> Result<(), InvalidMemoryAccess> {
        if !self.get_flag(F_USER_RING) {
            clear_flags!(self, F_MEMMAP_ENABLE);
            self.set_flag(F_MEMMAP_ENABLE, val);
            Ok(())
        } else {
            Err(InvalidMemoryAccess::UnprivilegedOpcode)
        }
    }

    fn set_interrupt_enable(&mut self, val: bool) -> Result<(), InvalidMemoryAccess> {
        if !self.get_flag(F_USER_RING) {
            clear_flags!(self, F_INTERRUPT_ENABLE);
            self.set_flag(F_INTERRUPT_ENABLE, val);
            Ok(())
        } else {
            Err(InvalidMemoryAccess::UnprivilegedOpcode)
        }
    }

    fn call(&mut self) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;

        let data = self.xs[R_BASE];
        for i in (0..4).rev() {
            self.write(self.xs[R_SP], (data >> (i * 8)) as u8)?;
            self.xs[R_SP] -= 1;
        }

        let data = self.xs[R_PC];
        for i in (0..4).rev() {
            self.write(self.xs[R_SP], (data >> (i * 8)) as u8)?;
            self.xs[R_SP] -= 1;
        }

        self.xs[R_BASE] = self.xs[R_SP];
        self.xs[R_PC] = addr;
        Ok(())
    }

    fn ret(&mut self) -> Result<(), InvalidMemoryAccess> {
        self.xs[R_PC] = 0;
        for i in 0..4 {
            self.xs[R_BASE] += 1;
            self.xs[R_PC] |= (self.read(self.xs[R_BASE])? as u32) << (8 * i);
        }

        let mut data = 0;
        for i in 0..4 {
            self.xs[R_BASE] += 1;
            data |= (self.read(self.xs[R_BASE])? as u32) << (8 * i);
        }

        self.xs[R_SP] = self.xs[R_BASE];
        self.xs[R_BASE] = data;

        Ok(())
    }

    fn branch_true(&mut self, flag: u32) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        if self.flags & (1 << flag) != 0 {
            self.xs[R_PC] = addr;
        }
        Ok(())
    }

    fn branch_false(&mut self, flag: u32) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        if self.flags & (1 << flag) == 0 {
            self.xs[R_PC] = addr;
        }
        Ok(())
    }

    fn load_lit_int(&mut self, x0: usize) -> Result<(), InvalidMemoryAccess> {
        let data = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        self.xs[x0] = data;
        self.update_flags_int(data);
        Ok(())
    }

    fn load_lit_float(&mut self, f0: usize) -> Result<(), InvalidMemoryAccess> {
        let data = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        let data = f32::from_bits(data);
        self.fs[f0] = data;
        self.update_flags_float(data);
        Ok(())
    }

    fn load_int(&mut self, x0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        let data = (self.read(addr)? as u32)
            | (self.read(addr + 1)? as u32) << 8
            | (self.read(addr + 2)? as u32) << 16
            | (self.read(addr + 3)? as u32) << 24;
        self.xs[x0] = data;
        self.update_flags_int(data);
        Ok(())
    }

    fn load_float(&mut self, f0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        let data = (self.read(addr)? as u32)
            | (self.read(addr + 1)? as u32) << 8
            | (self.read(addr + 2)? as u32) << 16
            | (self.read(addr + 3)? as u32) << 24;
        let data = f32::from_bits(data);
        self.fs[f0] = data;
        self.update_flags_float(data);
        Ok(())
    }

    fn iadd(&mut self, x0: usize, x1: usize) {
        let res = self.xs[x0] as u64 + self.xs[x1] as u64 + self.get_flag(F_CARRY) as u64;
        clear_flags!(self, F_ZERO, F_OVERFLOW, F_CARRY, F_NEGATIVE, F_PARITY);
        self.set_flag(F_ZERO, res as u32 == 0);
        self.set_flag(F_NEGATIVE, res & 0x80000000 != 0);
        self.set_flag(F_CARRY, res & 0x100000000 != 0);
        self.set_flag(
            F_OVERFLOW,
            self.xs[x0] & 0x80000000 == self.xs[x1] & 0x80000000
                && self.xs[x0] & 0x80000000 != res as u32 & 0x80000000,
        );
        self.set_flag(F_PARITY, res & 1 != 0);
        self.xs[x0] = res as u32;
    }

    fn isub(&mut self, x0: usize, x1: usize) {
        self.xs[x1] = !self.xs[x1];
        self.iadd(x0, x1);
        self.xs[x1] = !self.xs[x1];
    }

    fn update_flags_int(&mut self, x: u32) {
        clear_flags!(self, F_ZERO, F_NEGATIVE, F_PARITY);
        self.set_flag(F_ZERO, x == 0);
        self.set_flag(F_NEGATIVE, x & 0x80000000 != 0);
        self.set_flag(F_PARITY, x & 1 != 0);
    }

    fn imul(&mut self, x0: usize, x1: usize) {
        self.xs[x0] *= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn idiv(&mut self, x0: usize, x1: usize) {
        self.xs[x0] /= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn imod(&mut self, x0: usize, x1: usize) {
        self.xs[x0] %= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn update_flags_float(&mut self, x: f32) {
        clear_flags!(self, F_ZERO, F_NEGATIVE, F_NAN, F_INFINITE);
        self.set_flag(F_ZERO, x == 0.0);
        self.set_flag(F_NEGATIVE, x.is_sign_negative());
        self.set_flag(F_NAN, x.is_nan());
        self.set_flag(F_INFINITE, x.is_infinite());
    }

    fn fadd(&mut self, f0: usize, f1: usize) {
        self.fs[f0] += self.fs[f1];
        self.update_flags_float(self.fs[f0]);
    }

    fn fsub(&mut self, f0: usize, f1: usize) {
        self.fs[f0] -= self.fs[f1];
        self.update_flags_float(self.fs[f0]);
    }

    fn fmul(&mut self, f0: usize, f1: usize) {
        self.fs[f0] *= self.fs[f1];
        self.update_flags_float(self.fs[f0]);
    }

    fn fdiv(&mut self, f0: usize, f1: usize) {
        self.fs[f0] /= self.fs[f1];
        self.update_flags_float(self.fs[f0]);
    }

    fn bsl(&mut self, x0: usize, x1: usize) {
        let res = if self.xs[x1] < 32 {
            (self.xs[x0] as u64) << self.xs[x1] as u64
        } else {
            0
        } | self.get_flag(F_CARRY) as u64;

        clear_flags!(self, F_ZERO, F_CARRY, F_NEGATIVE, F_PARITY);
        self.set_flag(F_ZERO, res as u32 == 0);
        self.set_flag(F_NEGATIVE, res & 0x80000000 != 0);
        if self.xs[x1] == 1 {
            self.set_flag(F_CARRY, res & 0x100000000 != 0);
        }
        self.set_flag(F_PARITY, res & 1 != 0);
        self.xs[x0] = res as u32;
    }

    fn bsr(&mut self, x0: usize, x1: usize) {
        let res = if self.xs[x1] < 32 {
            (self.xs[x0] as u64) >> self.xs[x1] as u64
        } else {
            0
        } | self.get_flag(F_CARRY) as u64;

        clear_flags!(self, F_ZERO, F_CARRY, F_NEGATIVE, F_PARITY);
        self.set_flag(F_ZERO, res as u32 == 0);
        self.set_flag(F_NEGATIVE, res & 0x80000000 != 0);
        if self.xs[x1] == 1 {
            self.set_flag(F_CARRY, self.xs[x0] & 1 != 0);
        }
        self.set_flag(F_PARITY, res & 1 != 0);
        self.xs[x0] = res as u32;
    }

    fn and(&mut self, x0: usize, x1: usize) {
        self.xs[x0] &= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn or(&mut self, x0: usize, x1: usize) {
        self.xs[x0] |= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn xor(&mut self, x0: usize, x1: usize) {
        self.xs[x0] ^= self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn move_int(&mut self, x0: usize, x1: usize) {
        self.xs[x0] = self.xs[x1];
        self.update_flags_int(self.xs[x0]);
    }

    fn move_float(&mut self, x0: usize, x1: usize) {
        self.fs[x0] = self.fs[x1];
        self.update_flags_float(self.fs[x0]);
    }

    fn move_int_float(&mut self, x0: usize, f1: usize) {
        self.xs[x0] = (self.fs[f1] as i32) as u32;
        self.update_flags_int(self.xs[x0]);
    }

    fn move_float_int(&mut self, f0: usize, x1: usize) {
        self.fs[f0] = (self.xs[x1] as i32) as f32;
        self.update_flags_float(self.fs[f0]);
    }

    fn transmute_int_float(&mut self, x0: usize, f1: usize) {
        self.xs[x0] = self.fs[f1].to_bits();
        self.update_flags_int(self.xs[x0]);
    }

    fn transmute_float_int(&mut self, f0: usize, x1: usize) {
        self.fs[f0] = f32::from_bits(self.xs[x1]);
        self.update_flags_float(self.fs[f0]);
    }

    fn load_indirect_int(&mut self, x0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        let data = (self.read(addr)? as u32)
            | (self.read(addr + 1)? as u32) << 8
            | (self.read(addr + 2)? as u32) << 16
            | (self.read(addr + 3)? as u32) << 24;
        self.xs[x0] = data;
        self.update_flags_int(data);
        Ok(())
    }

    fn load_indirect_float(&mut self, f0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        let data = (self.read(addr)? as u32)
            | (self.read(addr + 1)? as u32) << 8
            | (self.read(addr + 2)? as u32) << 16
            | (self.read(addr + 3)? as u32) << 24;
        let data = f32::from_bits(data);
        self.fs[f0] = data;
        self.update_flags_float(data);
        Ok(())
    }

    fn store_indirect_int(&mut self, x0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        self.write(addr, self.xs[x0] as u8)?;
        self.write(addr + 1, (self.xs[x0] >> 8) as u8)?;
        self.write(addr + 2, (self.xs[x0] >> 16) as u8)?;
        self.write(addr + 3, (self.xs[x0] >> 24) as u8)
    }

    fn store_indirect_short(&mut self, x0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        self.write(addr, self.xs[x0] as u8)?;
        self.write(addr + 1, (self.xs[x0] >> 8) as u8)
    }

    fn store_indirect_byte(&mut self, x0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        self.write(addr, self.xs[x0] as u8)
    }

    fn store_indirect_float(&mut self, f0: usize, addr: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = self.xs[addr];
        let data = self.fs[f0].to_bits();
        self.write(addr, data as u8)?;
        self.write(addr + 1, (data >> 8) as u8)?;
        self.write(addr + 2, (data >> 16) as u8)?;
        self.write(addr + 3, (data >> 24) as u8)
    }

    fn store_int(&mut self, x0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        self.write(addr, self.xs[x0] as u8)?;
        self.write(addr + 1, (self.xs[x0] >> 8) as u8)?;
        self.write(addr + 2, (self.xs[x0] >> 16) as u8)?;
        self.write(addr + 3, (self.xs[x0] >> 24) as u8)
    }

    fn store_short(&mut self, x0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        self.write(addr, self.xs[x0] as u8)?;
        self.write(addr + 1, (self.xs[x0] >> 8) as u8)
    }

    fn store_byte(&mut self, x0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        self.write(addr, self.xs[x0] as u8)
    }

    fn store_float(&mut self, f0: usize) -> Result<(), InvalidMemoryAccess> {
        let addr = (self.exec()? as u32)
            | (self.exec()? as u32) << 8
            | (self.exec()? as u32) << 16
            | (self.exec()? as u32) << 24;
        let data = self.fs[f0].to_bits();
        self.write(addr, data as u8)?;
        self.write(addr + 1, (data >> 8) as u8)?;
        self.write(addr + 2, (data >> 16) as u8)?;
        self.write(addr + 3, (data >> 24) as u8)
    }

    fn privileged_move(&mut self, x0: usize, p: usize) -> Result<(), InvalidMemoryAccess> {
        if p as u32 & F_USER_RING != 0 {
            return Err(InvalidMemoryAccess::UnprivilegedOpcode);
        }

        match p {
            0 => self.flags = self.xs[x0],
            1 => self.memmap = self.xs[x0],
            2 => self.interrupt_mask = self.xs[x0] as u8,

            _ => ()
        }

        Ok(())
    }

    fn unprivileged_move(&mut self, p: usize, x0: usize) {
        match p {
            0 => self.xs[x0] = self.flags,
            1 => self.xs[x0] = self.memmap,
            2 => self.xs[x0] = self.interrupt_mask as u32,

            _ => ()
        }
    }

    fn exec(&mut self) -> Result<u8, InvalidMemoryAccess> {
        let addr = self.check_memory(self.xs[R_PC], EXEC)?;
        let res = self.addressing.read(addr);
        self.xs[R_PC] += 1;
        Ok(res)
    }

    fn read(&mut self, addr: u32) -> Result<u8, InvalidMemoryAccess> {
        let addr = self.check_memory(addr, READ)?;
        Ok(self.addressing.read(addr))
    }

    fn write(&mut self, addr: u32, data: u8) -> Result<(), InvalidMemoryAccess> {
        let addr = self.check_memory(addr, WRITE)?;
        self.addressing.write(addr, data);
        Ok(())
    }

    fn decode_instruction(&mut self) -> Result<(), InvalidMemoryAccess> {
        let opcode = self.exec()?;
        match opcode & 0xc0 {
            // 0b00xxxxxx -> no arguments
            0x00 => {
                match opcode & 0x3f {
                    // Branches
                    // Jumping is just mov x13, addr
                    // Takes in 32 bit data as an argument
                    0x00 => self.branch_true(F_ZERO)?,
                    0x01 => self.branch_true(F_OVERFLOW)?,
                    0x02 => self.branch_true(F_CARRY)?,
                    0x03 => self.branch_true(F_NEGATIVE)?,
                    0x04 => self.branch_true(F_PARITY)?,
                    0x05 => self.branch_true(F_NAN)?,
                    0x06 => self.branch_true(F_INFINITE)?,
                    0x07 => self.branch_true(F_MEMMAP_ENABLE)?,
                    0x08 => self.branch_false(F_ZERO)?,
                    0x09 => self.branch_false(F_OVERFLOW)?,
                    0x0a => self.branch_false(F_CARRY)?,
                    0x0b => self.branch_false(F_NEGATIVE)?,
                    0x0c => self.branch_false(F_PARITY)?,
                    0x0d => self.branch_false(F_NAN)?,
                    0x0e => self.branch_false(F_INFINITE)?,
                    0x0f => self.branch_false(F_MEMMAP_ENABLE)?,

                    // Setting and clearing flags
                    0x10 => self.set_carry(false),
                    0x11 => self.set_carry(true),
                    0x12 => self.set_memmap_enable(false)?,
                    0x13 => self.set_memmap_enable(true)?,
                    0x14 => self.set_interrupt_enable(false)?,
                    0x15 => self.set_interrupt_enable(true)?,
                    0x17 => self.set_user_ring(true)?,

                    0x18 => self.call()?,
                    0x19 => self.ret()?,

                    _ => (),
                }
            }

            // 0b01xxyyyy data -> one register argument and 32 bit data
            0x40 => {
                let data = opcode as usize & 0x0f;
                match opcode & 0x30 {
                    // Load literal
                    0x00 => self.load_lit_int(data)?,
                    0x10 => self.load_lit_float(data)?,

                    // Load memory address
                    0x20 => self.load_int(data)?,
                    0x30 => self.load_float(data)?,

                    _ => unreachable!("nya :("),
                }
            }

            // 0b10xxxxxx 0byyyyzzzz -> two register arguments
            0x80 => {
                let data = self.exec()?;
                let (fst, snd) = (((data & 0xf0) >> 4) as usize, (data & 0x0f) as usize);

                match opcode & 0x3f {
                    // Integer arithmetic
                    0x00 => self.iadd(fst, snd),
                    0x01 => self.isub(fst, snd),
                    0x02 => self.imul(fst, snd),
                    0x03 => self.idiv(fst, snd),
                    0x04 => self.imod(fst, snd),

                    // Floating point arithmetic
                    0x05 => self.fadd(fst, snd),
                    0x06 => self.fsub(fst, snd),
                    0x07 => self.fmul(fst, snd),
                    0x08 => self.fdiv(fst, snd),

                    // Bitwise operations
                    0x09 => self.bsl(fst, snd),
                    0x0a => self.bsr(fst, snd),
                    0x0b => self.and(fst, snd),
                    0x0c => self.or(fst, snd),
                    0x0d => self.xor(fst, snd),

                    // Move and transmute operations
                    0x0e => self.move_int(fst, snd),
                    0x0f => self.move_float(fst, snd),
                    0x10 => self.move_int_float(fst, snd),
                    0x11 => self.move_float_int(fst, snd),
                    0x12 => self.transmute_int_float(fst, snd),
                    0x13 => self.transmute_float_int(fst, snd),

                    // Load operations
                    0x14 => self.load_indirect_int(fst, snd)?,
                    0x15 => self.load_indirect_float(fst, snd)?,

                    // Store operations
                    0x16 => self.store_indirect_int(fst, snd)?,
                    0x17 => self.store_indirect_short(fst, snd)?,
                    0x18 => self.store_indirect_byte(fst, snd)?,
                    0x19 => self.store_indirect_float(fst, snd)?,

                    // Privileged move operations
                    0x1a => self.privileged_move(fst, snd)?,
                    0x1b => self.unprivileged_move(fst, snd),

                    _ => (),
                }
            }

            // 0b11xxyyyy data -> one register argument and 32 bit data
            0xc0 => {
                let data = opcode as usize & 0x0f;
                match opcode & 0x30 {
                    // Store at memory address
                    0x00 => self.store_int(data)?,
                    0x10 => self.store_short(data)?,
                    0x20 => self.store_byte(data)?,
                    0x30 => self.store_float(data)?,

                    _ => unreachable!("nya :("),
                }
            }

            _ => unreachable!("nya :("),
        }

        Ok(())
    }

    fn call_interrupt(&mut self, interrupt: u32) {
        let flags = self.flags;
        let int = self.xs[R_INT];
        let pc = self.xs[R_PC];
        self.xs[R_INT] = interrupt;

        if self.get_flag(F_USER_RING) {
            let sp = self.xs[R_SP];
            let base = self.xs[R_BASE];
            self.xs[R_SP] = self.system_sp;
            self.xs[R_BASE] = 0;

            for i in 4..8 {
            }
        } else {
        }
    }

    pub fn step(&mut self) {
        if !self.interrupt_queue.is_empty() && self.flags & F_INTERRUPT_ENABLE != 0 {
            let interrupt = self.interrupt_queue.pop_front().unwrap();
            self.call_interrupt(interrupt);

        } else {
            match self.decode_instruction() {
                Ok(_) => (),
                Err(e) => {
                    self.nmi(match e {
                        InvalidMemoryAccess::UsedFreePage => 0x00000000,
                        InvalidMemoryAccess::InvalidPermissions(_, _) => 0x00000001,
                        InvalidMemoryAccess::UnprivilegedOpcode => 0x00000002,
                    })
                }
            }
        }
    }

    pub fn irq(&mut self, id: u8) {
        if 1 << id & self.interrupt_mask != 0 {
            self.interrupt_queue.push_back(id as u32);
        }
    }

    pub fn nmi(&mut self, id: u32) {
        // self.interrupt_queue.push_back(id | 0x80000000);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_add() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Simple add
        cpu.xs[0] = 5;
        cpu.xs[1] = 10;
        cpu.iadd(0, 1);
        assert_eq!(cpu.xs[0], 15);
        assert!(!cpu.get_flag(F_CARRY));
        assert!(!cpu.get_flag(F_OVERFLOW));
        assert!(!cpu.get_flag(F_NEGATIVE));

        // Overflow
        cpu.xs[0] = (1 << 31) - 1;
        cpu.xs[1] = 1;
        cpu.iadd(0, 1);
        assert_eq!(cpu.xs[0], 0x80000000);
        assert!(!cpu.get_flag(F_CARRY));
        assert!(cpu.get_flag(F_OVERFLOW));
        assert!(cpu.get_flag(F_NEGATIVE));

        // Carry
        cpu.xs[0] = 0xffffffff;
        cpu.xs[1] = 0;
        cpu.set_flag(F_CARRY, true);
        cpu.iadd(0, 1);
        assert_eq!(cpu.xs[0], 0);
        assert!(cpu.get_flag(F_CARRY));
        assert!(!cpu.get_flag(F_OVERFLOW));
        assert!(!cpu.get_flag(F_NEGATIVE));
    }

    #[test]
    fn cpu_bsl() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Simple bitshift
        cpu.xs[0] = 3;
        cpu.xs[1] = 2;
        cpu.bsl(0, 1);
        assert_eq!(cpu.xs[0], 12);
        assert!(!cpu.get_flag(F_CARRY));

        // Overflow
        cpu.xs[0] = 3;
        cpu.xs[1] = 32;
        cpu.bsl(0, 1);
        assert_eq!(cpu.xs[0], 0);
        assert!(!cpu.get_flag(F_CARRY));

        // Carry
        cpu.xs[0] = 0xffffffff;
        cpu.xs[1] = 1;
        cpu.bsl(0, 1);
        assert_eq!(cpu.xs[0], 0xfffffffe);
        assert!(cpu.get_flag(F_CARRY));
    }

    #[test]
    fn cpu_load_int() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        cpu.addressing.memory[0xff00] = 0xd0;
        cpu.addressing.memory[0xff01] = 0xc0;
        cpu.addressing.memory[0xff02] = 0xb0;
        cpu.addressing.memory[0xff03] = 0xa0;

        // Load literal
        cpu.xs[R_PC] = 0xff00;
        cpu.load_lit_int(0).unwrap();
        assert_eq!(cpu.xs[0], 0xa0b0c0d0);

        // Set up memory
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;

        // Simple addressing
        cpu.xs[R_PC] = 0x00;
        cpu.load_int(1).unwrap();
        assert_eq!(cpu.xs[1], 0xa0b0c0d0);

        // Indirect addressing
        cpu.xs[1] = 0xff00;
        cpu.xs[0] = 0x00;
        cpu.load_indirect_int(0, 1).unwrap();
        assert_eq!(cpu.xs[0], 0xa0b0c0d0);
    }

    #[test]
    #[allow(clippy::float_cmp)]
    fn cpu_load_float() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        let data = 0.618f32.to_bits();
        cpu.addressing.memory[0xff00] = data as u8;
        cpu.addressing.memory[0xff01] = (data >> 8) as u8;
        cpu.addressing.memory[0xff02] = (data >> 16) as u8;
        cpu.addressing.memory[0xff03] = (data >> 24) as u8;

        // Load literal
        cpu.xs[R_PC] = 0xff00;
        cpu.load_lit_float(0).unwrap();
        assert_eq!(cpu.fs[0], 0.618);

        // Set up memory
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;

        // Simple addressing
        cpu.xs[R_PC] = 0x00;
        cpu.load_float(1).unwrap();
        assert_eq!(cpu.fs[1], 0.618);

        // Indirect addressing
        cpu.xs[1] = 0xff00;
        cpu.xs[0] = 0x00;
        cpu.load_indirect_float(0, 1).unwrap();
        assert_eq!(cpu.fs[0], 0.618);
    }

    #[test]
    fn cpu_store_int() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        cpu.xs[0] = 0xa0b0c0d0;
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;
        cpu.xs[R_PC] = 0x0000;

        // Simple addressing
        cpu.store_int(0).unwrap();
        assert_eq!(cpu.addressing.memory[0xff00], 0xd0);
        assert_eq!(cpu.addressing.memory[0xff01], 0xc0);
        assert_eq!(cpu.addressing.memory[0xff02], 0xb0);
        assert_eq!(cpu.addressing.memory[0xff03], 0xa0);

        // Indirect addressing
        cpu.xs[1] = 0xfe00;
        cpu.store_indirect_int(0, 1).unwrap();
        assert_eq!(cpu.addressing.memory[0xfe00], 0xd0);
        assert_eq!(cpu.addressing.memory[0xfe01], 0xc0);
        assert_eq!(cpu.addressing.memory[0xfe02], 0xb0);
        assert_eq!(cpu.addressing.memory[0xfe03], 0xa0);
    }

    #[test]
    fn cpu_store_short() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        cpu.xs[0] = 0xa0b0;
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;
        cpu.xs[R_PC] = 0x0000;

        // Simple addressing
        cpu.store_short(0).unwrap();
        assert_eq!(cpu.addressing.memory[0xff00], 0xb0);
        assert_eq!(cpu.addressing.memory[0xff01], 0xa0);

        // Indirect addressing
        cpu.xs[1] = 0xfe00;
        cpu.store_indirect_short(0, 1).unwrap();
        assert_eq!(cpu.addressing.memory[0xfe00], 0xb0);
        assert_eq!(cpu.addressing.memory[0xfe01], 0xa0);
    }

    #[test]
    fn cpu_store_byte() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        cpu.xs[0] = 0xa0;
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;
        cpu.xs[R_PC] = 0x0000;

        // Simple addressing
        cpu.store_byte(0).unwrap();
        assert_eq!(cpu.addressing.memory[0xff00], 0xa0);

        // Indirect addressing
        cpu.xs[1] = 0xfe00;
        cpu.store_indirect_int(0, 1).unwrap();
        assert_eq!(cpu.addressing.memory[0xfe00], 0xa0);
    }

    #[test]
    fn cpu_store_float() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up memory
        cpu.fs[0] = 0.618;
        cpu.addressing.memory[0x0000] = 0x00;
        cpu.addressing.memory[0x0001] = 0xff;
        cpu.addressing.memory[0x0002] = 0x00;
        cpu.addressing.memory[0x0003] = 0x00;
        cpu.xs[R_PC] = 0x0000;

        // Simple addressing
        cpu.store_float(0).unwrap();
        // 0x3f1e353f
        assert_eq!(cpu.addressing.memory[0xff00], 0x3f);
        assert_eq!(cpu.addressing.memory[0xff01], 0x35);
        assert_eq!(cpu.addressing.memory[0xff02], 0x1e);
        assert_eq!(cpu.addressing.memory[0xff03], 0x3f);

        // Indirect addressing
        cpu.xs[1] = 0xfe00;
        cpu.store_indirect_float(0, 1).unwrap();
        assert_eq!(cpu.addressing.memory[0xfe00], 0x3f);
        assert_eq!(cpu.addressing.memory[0xfe01], 0x35);
        assert_eq!(cpu.addressing.memory[0xfe02], 0x1e);
        assert_eq!(cpu.addressing.memory[0xfe03], 0x3f);
    }

    #[test]
    fn cpu_call_ret() {
        let mut cpu = Cpu::new(SimpleAddress::default());

        // Set up registers and memory
        cpu.xs[R_PC] = 0x1234;
        cpu.xs[R_BASE] = 0xbfff;
        cpu.xs[R_SP] = 0xbfc8;
        cpu.addressing.memory[0x1234] = 0x42;
        cpu.addressing.memory[0x1235] = 0xaf;
        cpu.addressing.memory[0x1236] = 0x00;
        cpu.addressing.memory[0x1237] = 0x00;

        // "Call" the function
        cpu.call().unwrap();
        assert_eq!(cpu.xs[R_PC], 0xaf42);
        assert_eq!(cpu.xs[R_BASE], 0xbfc0);

        // Simulate the stack being used and "return" from the function
        cpu.xs[R_SP] = 0xbf89;
        cpu.ret().unwrap();
        assert_eq!(cpu.xs[R_PC], 0x1238);
        assert_eq!(cpu.xs[R_BASE], 0xbfff);
        assert_eq!(cpu.xs[R_SP], 0xbfc8);
    }

    #[test]
    fn cpu_memmap() {
        let mut cpu = Cpu::new(SimpleAddress::default());
        cpu.flags |= 1 << F_MEMMAP_ENABLE;
        cpu.memmap = 0x1234;
        cpu.addressing.memory[0x1234] = 0x0a;
        cpu.addressing.memory[0x1235] = 0x0b;
        cpu.addressing.memory[0x1236] = 0x00;
        cpu.addressing.memory[0x1237] = 0x00;
        cpu.addressing.memory[0x0b0a] = 0x00;
        cpu.addressing.memory[0x0b0b] = 0xee;
        cpu.addressing.memory[0x0b0c] = 0x00;
        cpu.addressing.memory[0x0b0d] = 0xe0;
        cpu.write(0x000000bc, 0x42).unwrap();
        assert_eq!(cpu.addressing.memory[0x0000eebc], 0x42);
        assert_eq!(cpu.read(0xbc).unwrap(), 0x42);
        assert!(cpu.exec().is_err());
    }
}
