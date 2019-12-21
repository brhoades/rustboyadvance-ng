use bit::BitIndex;

use super::dma::DmaController;
use super::iodev::consts::*;
use super::iodev::io_reg_string;
use crate::AudioInterface;

mod fifo;
use fifo::SoundFifo;

const DMG_RATIOS: [f32; 4] = [0.25, 0.5, 1.0, 0.0];
const DMA_RATIOS: [f32; 2] = [0.5, 1.0];
const DMA_TIMERS: [usize; 2] = [0, 1];
const DUTY_RATIOS: [f32; 4] = [0.125, 0.25, 0.5, 0.75];

#[derive(Debug)]
struct NoiseChannel {}

#[derive(Debug)]
struct DmaSoundChannel {
    value: i8,
    volume: f32,
    enable_right: bool,
    enable_left: bool,
    timer_select: usize,
    fifo: SoundFifo,
}

impl Default for DmaSoundChannel {
    fn default() -> DmaSoundChannel {
        DmaSoundChannel {
            volume: DMA_RATIOS[0],
            value: 0,
            enable_right: false,
            enable_left: false,
            timer_select: 0,
            fifo: SoundFifo::new(),
        }
    }
}

const REG_FIFO_A_L: u32 = REG_FIFO_A;
const REG_FIFO_A_H: u32 = REG_FIFO_A + 2;

const REG_FIFO_B_L: u32 = REG_FIFO_B;
const REG_FIFO_B_H: u32 = REG_FIFO_B + 2;

#[derive(Debug)]
pub struct SoundController {
    sample_rate_to_cpu_freq: usize, // how many "cycles" are a sample?
    last_sample_cycles: usize,      // cycles count when we last provided a new sample.

    mse: bool,

    left_volume: usize,
    left_sqr1: bool,
    left_sqr2: bool,
    left_wave: bool,
    left_noise: bool,

    right_volume: usize,
    right_sqr1: bool,
    right_sqr2: bool,
    right_wave: bool,
    right_noise: bool,

    dmg_volume_ratio: f32,

    sqr1_rate: usize,
    sqr1_timed: bool,
    sqr1_length: f32,
    sqr1_duty: f32,
    sqr1_step_time: usize,
    sqr1_step_increase: bool,
    sqr1_initial_vol: usize,
    sqr1_cur_vol: usize,

    sound_bias: u16,

    dma_sound: [DmaSoundChannel; 2],

    pub output_buffer: Vec<i8>,
}

impl SoundController {
    pub fn new() -> SoundController {
        SoundController {
            sample_rate_to_cpu_freq: 12345,
            last_sample_cycles: 0,
            mse: false,
            left_volume: 0,
            left_sqr1: false,
            left_sqr2: false,
            left_wave: false,
            left_noise: false,
            right_volume: 0,
            right_sqr1: false,
            right_sqr2: false,
            right_wave: false,
            right_noise: false,
            dmg_volume_ratio: 0.0,
            sqr1_rate: 0,
            sqr1_timed: false,
            sqr1_length: 0.0,
            sqr1_duty: DUTY_RATIOS[0],
            sqr1_step_time: 0,
            sqr1_step_increase: false,
            sqr1_initial_vol: 0,
            sqr1_cur_vol: 0,
            sound_bias: 0x200,
            dma_sound: [Default::default(), Default::default()],

            output_buffer: Vec::with_capacity(32),
        }
    }

    pub fn handle_read(&self, io_addr: u32) -> u16 {
        let value = match io_addr {
            REG_SOUNDCNT_X => cbit(7, self.mse),
            REG_SOUNDCNT_L => {
                self.left_volume as u16
                    | (self.right_volume as u16) << 4
                    | cbit(8, self.left_sqr1)
                    | cbit(9, self.left_sqr2)
                    | cbit(10, self.left_wave)
                    | cbit(11, self.left_noise)
                    | cbit(12, self.right_sqr1)
                    | cbit(13, self.right_sqr2)
                    | cbit(14, self.right_wave)
                    | cbit(15, self.right_noise)
            }

            REG_SOUNDCNT_H => {
                DMG_RATIOS
                    .iter()
                    .position(|&f| f == self.dmg_volume_ratio)
                    .expect("bad dmg_volume_ratio!") as u16
                    | DMA_RATIOS
                        .iter()
                        .position(|&f| f == self.dma_sound[0].volume)
                        .unwrap() as u16
                    | DMA_RATIOS
                        .iter()
                        .position(|&f| f == self.dma_sound[1].volume)
                        .unwrap() as u16
                    | cbit(8, self.dma_sound[0].enable_right)
                    | cbit(9, self.dma_sound[0].enable_left)
                    | cbit(10, self.dma_sound[0].timer_select != 0)
                    | cbit(12, self.dma_sound[1].enable_right)
                    | cbit(13, self.dma_sound[1].enable_left)
                    | cbit(14, self.dma_sound[1].timer_select != 0)
            }

            REG_SOUNDBIAS => self.sound_bias,

            _ => {
                println!(
                    "Unimplemented read from {:x} {}",
                    io_addr,
                    io_reg_string(io_addr)
                );
                0
            }
        };
        println!(
            "Read {} ({:08x}) = {:04x}",
            io_reg_string(io_addr),
            io_addr,
            value
        );
        value
    }

    pub fn handle_write(&mut self, io_addr: u32, value: u16) {
        if io_addr == REG_SOUNDCNT_X {
            if value & bit(7) != 0 {
                if !self.mse {
                    println!("MSE enabled!");
                    self.mse = true;
                }
            } else {
                if self.mse {
                    println!("MSE disabled!");
                    self.mse = false;
                }
            }

            // other fields of this register are read-only anyway, ignore them.
            return;
        }

        if !self.mse {
            // println!("MSE disabled, refusing to write");
            return;
        }

        match io_addr {
            REG_SOUNDCNT_L => {
                self.left_volume = value.bit_range(0..2) as usize;
                self.right_volume = value.bit_range(4..6) as usize;
                self.left_sqr1 = value.bit(8);
                self.left_sqr2 = value.bit(9);
                self.left_wave = value.bit(10);
                self.left_noise = value.bit(11);
                self.right_sqr1 = value.bit(12);
                self.right_sqr2 = value.bit(13);
                self.right_wave = value.bit(14);
                self.right_noise = value.bit(15);
            }

            REG_SOUNDCNT_H => {
                self.dmg_volume_ratio = DMG_RATIOS[value.bit_range(0..1) as usize];
                self.dma_sound[0].volume = DMA_RATIOS[value.bit(2) as usize];
                self.dma_sound[1].volume = DMA_RATIOS[value.bit(3) as usize];
                self.dma_sound[0].enable_right = value.bit(8);
                self.dma_sound[0].enable_left = value.bit(9);
                self.dma_sound[0].timer_select = DMA_TIMERS[value.bit(10) as usize];
                self.dma_sound[1].enable_right = value.bit(12);
                self.dma_sound[1].enable_left = value.bit(13);
                self.dma_sound[1].timer_select = DMA_TIMERS[value.bit(14) as usize];

                if value.bit(11) {
                    self.dma_sound[0].fifo.reset();
                }
                if value.bit(15) {
                    self.dma_sound[1].fifo.reset();
                }
            }

            REG_SOUND1CNT_H => {
                self.sqr1_length = (64 - value.bit_range(0..5) as usize) as f32 / 256.0;
                self.sqr1_duty = DUTY_RATIOS[value.bit_range(6..7) as usize];
                self.sqr1_step_time = value.bit_range(8..10) as usize;
                self.sqr1_step_increase = value.bit(11);
                self.sqr1_initial_vol = value.bit_range(12..15) as usize;
            }

            REG_SOUND1CNT_X => {
                self.sqr1_rate = value.bit_range(0..10) as usize;
                self.sqr1_timed = value.bit(14);
                if value.bit(15) {
                    self.sqr1_cur_vol = self.sqr1_initial_vol;
                }
            }

            REG_FIFO_A_L | REG_FIFO_A_H => {
                self.dma_sound[0].fifo.write(((value >> 8) & 0xff) as i8);
                self.dma_sound[0].fifo.write((value & 0xff) as i8);
            }

            REG_FIFO_B_L | REG_FIFO_B_H => {
                self.dma_sound[1].fifo.write(((value >> 8) & 0xff) as i8);
                self.dma_sound[1].fifo.write((value & 0xff) as i8);
            }

            REG_SOUNDBIAS => self.sound_bias = value & 0xc3fe,

            _ => {
                println!(
                    "Unimplemented write to {:x} {}",
                    io_addr,
                    io_reg_string(io_addr)
                );
            }
        }
    }

    pub fn handle_timer_overflow(
        &mut self,
        dmac: &mut DmaController,
        timer_id: usize,
        num_overflows: usize,
    ) {
        if !self.mse {
            return;
        }
        // TODO - play sound ?

        const FIFO_INDEX_TO_REG: [u32; 2] = [REG_FIFO_A, REG_FIFO_B];
        for fifo in 0..2 {
            let channel = &mut self.dma_sound[fifo];

            if timer_id == channel.timer_select {
                channel.value = channel.fifo.read();
                if channel.fifo.count() <= 16 {
                    dmac.notify_sound_fifo(FIFO_INDEX_TO_REG[fifo]);
                }
            }
        }
    }

    fn sample_rate(&self) -> i32 {
        let resolution = self.sound_bias.bit_range(14..16) as usize;
        (32768 << resolution) as i32
    }

    pub fn update(&mut self, cycles: usize, audio_device: &mut dyn AudioInterface) {
        let resolution = self.sound_bias.bit_range(14..16) as usize;
        let cycles_per_sample = 512 >> resolution;
        while cycles - self.last_sample_cycles >= cycles_per_sample {
            self.last_sample_cycles += cycles_per_sample;

            let mut sample = (0, 0);

            for i in 0..2 {
                let channel = &self.dma_sound[i];
                if channel.enable_left {
                    sample.0 += (channel.value as i16) << 8;
                }
                if channel.enable_right {
                    sample.1 += (channel.value as i16) << 8;
                }
            }
            audio_device.play(&[sample.0, sample.1]);
        }
    }
}

// TODO move
fn cbit(idx: u8, value: bool) -> u16 {
    if value {
        1 << idx
    } else {
        0
    }
}

// TODO mvoe
fn bit(idx: u8) -> u16 {
    1 << idx
}

fn rate_to_freq(rate: u16) -> usize {
    assert!(rate < 2048);

    (2 << 17) as usize / (2048 - rate) as usize
}
