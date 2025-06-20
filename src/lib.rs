// OVP - v2
use std::os::unix::io::RawFd;
use std::ptr;
use std::mem;
use std::ffi::CString;
use std::sync::Arc;
use std::thread;
use std::sync::mpsc::{channel, Receiver, Sender};

// Raw socket constants
const AF_PACKET: i32 = 17;
const SOCK_RAW: i32 = 3;
const ETH_P_ALL: u16 = 0x0003;
const SOL_PACKET: i32 = 263;
const PACKET_ADD_MEMBERSHIP: i32 = 1;
const PACKET_MR_PROMISC: i32 = 1;

// OVP Protocol Magic
const OVP_MAGIC: u32 = 0xDEADBEEF;

// Pre-allocated buffer sizes for zero-allocation hot paths
const MAX_FRAME_SIZE: usize = 1500; // Standard MTU
const RECV_BUFFER_SIZE: usize = 65536;

// Core types
pub type DroneId = u64;

#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct OmegaFrame {
    magic: u32,
    target_count: u32,
    payload_len: u32,
    // Dynamic data follows: targets[], payload[]
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct PacketMreq {
    mr_ifindex: i32,
    mr_type: u16,
    mr_alen: u16,
    mr_address: [u8; 8],
}

#[repr(C, packed)]
#[derive(Copy, Clone)]
struct SockaddrLl {
    sll_family: u16,
    sll_protocol: u16,
    sll_ifindex: i32,
    sll_hatype: u16,
    sll_pkttype: u8,
    sll_halen: u8,
    sll_addr: [u8; 8],
}

unsafe extern "C" {
    unsafe fn socket(domain: i32, type_: i32, protocol: i32) -> i32;
    unsafe fn setsockopt(sockfd: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    unsafe fn bind(sockfd: i32, addr: *const SockaddrLl, addrlen: u32) -> i32;
    unsafe fn sendto(sockfd: i32, buf: *const u8, len: usize, flags: i32, 
              dest_addr: *const SockaddrLl, addrlen: u32) -> isize;
    unsafe fn recvfrom(sockfd: i32, buf: *mut u8, len: usize, flags: i32,
                src_addr: *mut SockaddrLl, addrlen: *mut u32) -> isize;
    unsafe fn if_nametoindex(ifname: *const i8) -> u32;
    unsafe fn close(fd: i32) -> i32;
}

pub struct OmegaSocket {
    raw_fd: RawFd,
    interface_index: u32,
    // Pre-allocated buffers for zero-allocation hot paths
    send_buffer: Box<[u8; MAX_FRAME_SIZE]>,
    recv_buffer: Box<[u8; RECV_BUFFER_SIZE]>,
    dest_addr: SockaddrLl, // Pre-computed destination
}

impl OmegaSocket {
    #[inline]
    pub fn new(interface: &str) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            // Create raw packet socket
            let fd = socket(AF_PACKET, SOCK_RAW, (ETH_P_ALL as u16).to_be() as i32);
            if fd < 0 {
                return Err("Failed to create raw socket".into());
            }

            // Get interface index
            let if_name = CString::new(interface)?;
            let if_index = if_nametoindex(if_name.as_ptr());
            if if_index == 0 {
                close(fd);
                return Err("Interface not found".into());
            }

            // Enable promiscuous mode - RECEIVE EVERYTHING
            let mreq = PacketMreq {
                mr_ifindex: if_index as i32,
                mr_type: PACKET_MR_PROMISC as u16,
                mr_alen: 0,
                mr_address: [0; 8],
            };

            let ret = setsockopt(
                fd,
                SOL_PACKET,
                PACKET_ADD_MEMBERSHIP,
                ptr::addr_of!(mreq) as *const u8,
                mem::size_of::<PacketMreq>() as u32,
            );

            if ret < 0 {
                close(fd);
                return Err("Failed to set promiscuous mode".into());
            }

            // Bind to interface
            let addr = SockaddrLl {
                sll_family: AF_PACKET as u16,
                sll_protocol: (ETH_P_ALL as u16).to_be(),
                sll_ifindex: if_index as i32,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 0,
                sll_addr: [0; 8],
            };

            let ret = bind(fd, ptr::addr_of!(addr), mem::size_of::<SockaddrLl>() as u32);
            if ret < 0 {
                close(fd);
                return Err("Failed to bind socket".into());
            }

            // Pre-compute destination address for broadcasts
            let dest_addr = SockaddrLl {
                sll_family: AF_PACKET as u16,
                sll_protocol: (ETH_P_ALL as u16).to_be(),
                sll_ifindex: if_index as i32,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 6, // MAC address length
                sll_addr: [0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0, 0], // Broadcast MAC
            };

            Ok(OmegaSocket {
                raw_fd: fd,
                interface_index: if_index,
                send_buffer: Box::new([0u8; MAX_FRAME_SIZE]),
                recv_buffer: Box::new([0u8; RECV_BUFFER_SIZE]),
                dest_addr,
            })
        }
    }

    // THE MAGIC: One spherical emit to rule them all - ZERO ALLOCATION HOT PATH
    #[inline(always)]
    pub fn emit_frame(&mut self, frame_data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            let sent = sendto(
                self.raw_fd,
                frame_data.as_ptr(),
                frame_data.len(),
                0,
                ptr::addr_of!(self.dest_addr),
                mem::size_of::<SockaddrLl>() as u32,
            );

            if sent < 0 {
                return Err("Failed to send frame".into());
            }

            Ok(())
        }
    }

    // Receive ANY frame in range - ZERO ALLOCATION HOT PATH
    #[inline(always)]
    pub fn receive_frame(&mut self) -> Result<&[u8], Box<dyn std::error::Error>> {
        unsafe {
            let mut src_addr: SockaddrLl = mem::zeroed();
            let mut addr_len = mem::size_of::<SockaddrLl>() as u32;

            let received = recvfrom(
                self.raw_fd,
                self.recv_buffer.as_mut_ptr(),
                self.recv_buffer.len(),
                0,
                ptr::addr_of_mut!(src_addr),
                ptr::addr_of_mut!(addr_len),
            );

            if received < 0 {
                return Err("Failed to receive frame".into());
            }

            Ok(&self.recv_buffer[..received as usize])
        }
    }

    // Zero-allocation frame building directly in send buffer
    #[inline(always)]
    pub fn build_and_emit(&mut self, targets: &[DroneId], payload: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let header_size = 12; // magic + target_count + payload_len
        let targets_size = targets.len() * 8;
        let total_size = header_size + targets_size + payload.len();
        
        if total_size > MAX_FRAME_SIZE {
            return Err("Frame too large".into());
        }

        unsafe {
            let buf = self.send_buffer.as_mut_ptr();
            
            // Write header directly to buffer
            ptr::write_unaligned(buf as *mut u32, OVP_MAGIC.to_le());
            ptr::write_unaligned(buf.add(4) as *mut u32, (targets.len() as u32).to_le());
            ptr::write_unaligned(buf.add(8) as *mut u32, (payload.len() as u32).to_le());
            
            // Write targets
            let mut offset = 12;
            for &target in targets {
                ptr::write_unaligned(buf.add(offset) as *mut u64, target.to_le());
                offset += 8;
            }
            
            // Write payload
            ptr::copy_nonoverlapping(payload.as_ptr(), buf.add(offset), payload.len());
            
            // Emit directly from buffer
            let sent = sendto(
                self.raw_fd,
                buf,
                total_size,
                0,
                ptr::addr_of!(self.dest_addr),
                mem::size_of::<SockaddrLl>() as u32,
            );

            if sent < 0 {
                return Err("Failed to send frame".into());
            }
        }

        Ok(())
    }
}

impl Drop for OmegaSocket {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            close(self.raw_fd);
        }
    }
}

// THE CLIENT API - Optimized for speed
pub struct OVP {
    socket: OmegaSocket, // No Arc needed if single-threaded per instance
    my_drone_id: DroneId,
    receiver_handle: Option<thread::JoinHandle<()>>,
    message_rx: Receiver<Vec<u8>>,
}

impl OVP {
    #[inline]
    pub fn new(interface: &str, my_drone_id: DroneId) -> Result<Self, Box<dyn std::error::Error>> {
        let mut socket = OmegaSocket::new(interface)?;
        let (tx, rx) = channel();
        
        // For multi-threaded usage, you'd need to split socket into send/recv parts
        // This simplified version assumes single-threaded usage for maximum performance
        
        Ok(OVP {
            socket,
            my_drone_id,
            receiver_handle: None,
            message_rx: rx,
        })
    }

    // THE ONLY API METHOD - Pure volumetric power - ZERO ALLOCATION HOT PATH
    #[inline(always)]
    pub fn emit(&mut self, neighbours: Option<&[DroneId]>, payload: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let targets = neighbours.unwrap_or(&[]);
        
        // ONE SPHERICAL EMIT - REACHES ALL IN RANGE - ZERO ALLOCATION
        self.socket.build_and_emit(targets, payload)?;
        
        Ok(())
    }

    // Optimized parsing with early returns
    #[inline(always)]
    pub fn try_receive(&mut self) -> Option<&[u8]> {
        match self.socket.receive_frame() {
            Ok(frame) => parse_ovp_frame_fast(frame, self.my_drone_id),
            Err(_) => None,
        }
    }
}

// Ultra-fast frame parsing with early exits and no allocations
#[inline(always)]
fn parse_ovp_frame_fast(frame: &[u8], my_id: DroneId) -> Option<&[u8]> {
    if frame.len() < 12 { 
        return None; 
    }
    
    // Check magic with unsafe unaligned read for speed
    unsafe {
        let magic = ptr::read_unaligned(frame.as_ptr() as *const u32);
        if u32::from_le(magic) != OVP_MAGIC { 
            return None; 
        }
        
        let target_count = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(4) as *const u32)) as usize;
        let payload_len = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(8) as *const u32)) as usize;
        
        let targets_start = 12;
        let targets_end = targets_start + (target_count * 8);
        let payload_start = targets_end;
        let payload_end = payload_start + payload_len;
        
        if frame.len() < payload_end { 
            return None; 
        }
        
        // Check if message is for me - fast path for broadcasts
        if target_count == 0 {
            return Some(&frame[payload_start..payload_end]);
        }
        
        // Fast target checking with unaligned reads
        let targets_ptr = frame.as_ptr().add(targets_start) as *const u64;
        for i in 0..target_count {
            let target_id = u64::from_le(ptr::read_unaligned(targets_ptr.add(i)));
            if target_id == my_id {
                return Some(&frame[payload_start..payload_end]);
            }
        }
        
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_zero_alloc_frame_ops() {
        let mut socket = OmegaSocket::new("lo").unwrap();
        let targets = vec![1, 2, 3];
        let payload = b"Hello volumetric world!";
        
        // This should not allocate in the hot path
        socket.build_and_emit(&targets, payload).unwrap();
    }
}