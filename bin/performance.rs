use std::thread;
use std::time::{Duration, Instant};

// Import both versions - assume old version is in ovp_old module
// and new optimized version is in ovp_new module
mod ovp_old {
    // Your original code here - copy the original structs and functions
    use super::*;
    use std::ffi::CString;
    use std::mem;
    use std::os::unix::io::RawFd;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

    // Raw socket constants
    const AF_PACKET: i32 = 17;
    const SOCK_RAW: i32 = 3;
    const ETH_P_ALL: u16 = 0x0003;
    const SOL_PACKET: i32 = 263;
    const PACKET_ADD_MEMBERSHIP: i32 = 1;
    const PACKET_MR_PROMISC: i32 = 1;

    // OVP Protocol Magic
    const OVP_MAGIC: u32 = 0xDEADBEEF;

    // Core types
    pub type DroneId = u64;

    #[repr(C, packed)]
    #[derive(Debug)]
    pub struct OmegaFrame {
        magic: u32,
        target_count: u32,
        payload_len: u32,
        // Dynamic data follows: targets[], payload[]
    }

    #[repr(C)]
    struct PacketMreq {
        mr_ifindex: i32,
        mr_type: u16,
        mr_alen: u16,
        mr_address: [u8; 8],
    }

    #[repr(C)]
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
        unsafe fn setsockopt(
            sockfd: i32,
            level: i32,
            optname: i32,
            optval: *const u8,
            optlen: u32,
        ) -> i32;
        unsafe fn bind(sockfd: i32, addr: *const SockaddrLl, addrlen: u32) -> i32;
        unsafe fn sendto(
            sockfd: i32,
            buf: *const u8,
            len: usize,
            flags: i32,
            dest_addr: *const SockaddrLl,
            addrlen: u32,
        ) -> isize;
        unsafe fn recvfrom(
            sockfd: i32,
            buf: *mut u8,
            len: usize,
            flags: i32,
            src_addr: *mut SockaddrLl,
            addrlen: *mut u32,
        ) -> isize;
        unsafe fn if_nametoindex(ifname: *const i8) -> u32;
        unsafe fn close(fd: i32) -> i32;
    }

    pub struct OmegaSocket {
        raw_fd: RawFd,
        interface_index: u32,
    }

    impl OmegaSocket {
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
                    &mreq as *const _ as *const u8,
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

                let ret = bind(fd, &addr, mem::size_of::<SockaddrLl>() as u32);
                if ret < 0 {
                    close(fd);
                    return Err("Failed to bind socket".into());
                }

                Ok(OmegaSocket {
                    raw_fd: fd,
                    interface_index: if_index,
                })
            }
        }

        // THE MAGIC: One spherical emit to rule them all
        pub fn emit_frame(&self, frame_data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
            unsafe {
                let dest_addr = SockaddrLl {
                    sll_family: AF_PACKET as u16,
                    sll_protocol: (ETH_P_ALL as u16).to_be(),
                    sll_ifindex: self.interface_index as i32,
                    sll_hatype: 0,
                    sll_pkttype: 0,
                    sll_halen: 6,        // MAC address length
                    sll_addr: [0xFF; 8], // Broadcast MAC
                };

                let sent = sendto(
                    self.raw_fd,
                    frame_data.as_ptr(),
                    frame_data.len(),
                    0,
                    &dest_addr,
                    mem::size_of::<SockaddrLl>() as u32,
                );

                if sent < 0 {
                    return Err("Failed to send frame".into());
                }

                Ok(())
            }
        }

        // Receive ANY frame in range
        pub fn receive_frame(&self) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
            unsafe {
                let mut buffer = vec![0u8; 65536]; // Max ethernet frame
                let mut src_addr: SockaddrLl = mem::zeroed();
                let mut addr_len = mem::size_of::<SockaddrLl>() as u32;

                let received = recvfrom(
                    self.raw_fd,
                    buffer.as_mut_ptr(),
                    buffer.len(),
                    0,
                    &mut src_addr,
                    &mut addr_len,
                );

                if received < 0 {
                    return Err("Failed to receive frame".into());
                }

                buffer.truncate(received as usize);
                Ok(buffer)
            }
        }
    }

    impl Drop for OmegaSocket {
        fn drop(&mut self) {
            unsafe {
                close(self.raw_fd);
            }
        }
    }

    // THE CLIENT API - Simple as fuck
    pub struct OVP {
        socket: Arc<OmegaSocket>,
        my_drone_id: DroneId,
        receiver_handle: Option<thread::JoinHandle<()>>,
        message_rx: Receiver<Vec<u8>>,
    }

    impl Clone for OVP {
        fn clone(&self) -> Self {
            // Create a new OVP instance with the same socket and drone ID
            // Note: This creates a new receiver channel since we can't clone the existing one
            let (tx, rx) = std::sync::mpsc::channel();

            // Create a new OVP instance with the cloned socket and new receiver
            OVP {
                socket: self.socket.clone(),
                my_drone_id: self.my_drone_id,
                receiver_handle: None, // New instance won't have a receiver handle
                message_rx: rx,        // New receiver channel
            }
        }
    }

    impl OVP {
        pub fn new(
            interface: &str,
            my_drone_id: DroneId,
        ) -> Result<Self, Box<dyn std::error::Error>> {
            let socket = Arc::new(OmegaSocket::new(interface)?);
            let (tx, rx) = channel();

            // Start background receiver thread
            let socket_clone = Arc::clone(&socket);
            let handle = thread::spawn(move || {
                loop {
                    match socket_clone.receive_frame() {
                        Ok(frame) => {
                            if let Some(payload) = parse_ovp_frame(&frame, my_drone_id) {
                                let _ = tx.send(payload);
                            }
                        }
                        Err(_) => continue,
                    }
                }
            });

            Ok(OVP {
                socket,
                my_drone_id,
                receiver_handle: Some(handle),
                message_rx: rx,
            })
        }

        // THE ONLY API METHOD - Pure volumetric power
        pub fn emit(
            &self,
            neighbours: Option<Vec<DroneId>>,
            payload: Vec<u8>,
        ) -> Result<(), Box<dyn std::error::Error>> {
            let targets = neighbours.unwrap_or_else(Vec::new);
            let frame = build_ovp_frame(&targets, &payload);

            // ONE SPHERICAL EMIT - REACHES ALL IN RANGE
            self.socket.emit_frame(&frame)?;

            Ok(())
        }

        // Non-blocking message receive
        pub fn try_receive(&self) -> Option<Vec<u8>> {
            self.message_rx.try_recv().ok()
        }
    }

    // Frame building/parsing helpers
    fn build_ovp_frame(targets: &[DroneId], payload: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();

        // Header
        frame.extend_from_slice(&OVP_MAGIC.to_le_bytes());
        frame.extend_from_slice(&(targets.len() as u32).to_le_bytes());
        frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());

        // Targets
        for &target in targets {
            frame.extend_from_slice(&target.to_le_bytes());
        }

        // Payload
        frame.extend_from_slice(payload);

        frame
    }

    fn parse_ovp_frame(frame: &[u8], my_id: DroneId) -> Option<Vec<u8>> {
        if frame.len() < 12 {
            return None;
        }

        // Check magic
        let magic = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
        if magic != OVP_MAGIC {
            return None;
        }

        let target_count = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
        let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;

        let targets_start = 12;
        let targets_end = targets_start + (target_count * 8);
        let payload_start = targets_end;
        let payload_end = payload_start + payload_len;

        if frame.len() < payload_end {
            return None;
        }

        // Check if message is for me
        if target_count > 0 {
            let mut found = false;
            for i in 0..target_count {
                let offset = targets_start + (i * 8);
                let target_id = u64::from_le_bytes([
                    frame[offset],
                    frame[offset + 1],
                    frame[offset + 2],
                    frame[offset + 3],
                    frame[offset + 4],
                    frame[offset + 5],
                    frame[offset + 6],
                    frame[offset + 7],
                ]);
                if target_id == my_id {
                    found = true;
                    break;
                }
            }
            if !found {
                return None;
            }
        }

        // Extract payload
        Some(frame[payload_start..payload_end].to_vec())
    }
}

mod ovp_new {
    // Optimized version
    use std::ffi::CString;
    use std::mem;
    use std::os::unix::io::RawFd;
    use std::ptr;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::thread;

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
        unsafe fn setsockopt(
            sockfd: i32,
            level: i32,
            optname: i32,
            optval: *const u8,
            optlen: u32,
        ) -> i32;
        unsafe fn bind(sockfd: i32, addr: *const SockaddrLl, addrlen: u32) -> i32;
        unsafe fn sendto(
            sockfd: i32,
            buf: *const u8,
            len: usize,
            flags: i32,
            dest_addr: *const SockaddrLl,
            addrlen: u32,
        ) -> isize;
        unsafe fn recvfrom(
            sockfd: i32,
            buf: *mut u8,
            len: usize,
            flags: i32,
            src_addr: *mut SockaddrLl,
            addrlen: *mut u32,
        ) -> isize;
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
        pub fn build_and_emit(
            &mut self,
            targets: &[DroneId],
            payload: &[u8],
        ) -> Result<(), Box<dyn std::error::Error>> {
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
        pub fn new(
            interface: &str,
            my_drone_id: DroneId,
        ) -> Result<Self, Box<dyn std::error::Error>> {
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
        pub fn emit(
            &mut self,
            neighbours: Option<&[DroneId]>,
            payload: &[u8],
        ) -> Result<(), Box<dyn std::error::Error>> {
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

            let target_count =
                u32::from_le(ptr::read_unaligned(frame.as_ptr().add(4) as *const u32)) as usize;
            let payload_len =
                u32::from_le(ptr::read_unaligned(frame.as_ptr().add(8) as *const u32)) as usize;

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
}

// Benchmark configuration
const BENCHMARK_ITERATIONS: usize = 10_000;
const PAYLOAD_SIZE: usize = 64; // Typical drone telemetry size
const TARGET_COUNT: usize = 5; // Typical swarm neighbors

fn main() {
    println!("🚁 OVP Protocol Performance Benchmark");
    println!("═══════════════════════════════════════");
    println!("Iterations: {}", BENCHMARK_ITERATIONS);
    println!("Payload Size: {} bytes", PAYLOAD_SIZE);
    println!("Target Count: {}", TARGET_COUNT);
    println!();

    // Test data
    let targets: Vec<u64> = (1..=TARGET_COUNT as u64).collect();
    let payload = vec![0x42u8; PAYLOAD_SIZE];
    let test_drone_id = 3u64; // One of the targets

    // Benchmark frame building/parsing (without actual network I/O)
    benchmark_frame_operations(&targets, &payload, test_drone_id);

    // If you want to test actual network I/O (requires interface)
    // benchmark_network_io(&targets, &payload, test_drone_id);

    println!("\n🎯 Benchmark completed!");
}

fn benchmark_frame_operations(targets: &[u64], payload: &[u8], test_drone_id: u64) {
    println!("📊 Frame Building & Parsing Benchmark");
    println!("─────────────────────────────────────");

    // OLD VERSION - Frame building
    let start = Instant::now();
    for _ in 0..BENCHMARK_ITERATIONS {
        let frame = build_ovp_frame_old(targets, payload);
        // Prevent optimization from eliminating the work
        std::hint::black_box(frame);
    }
    let old_build_time = start.elapsed();

    // NEW VERSION - Frame building (simulated without actual socket)
    let start = Instant::now();
    let mut buffer = vec![0u8; 1500]; // Simulate send buffer
    for _ in 0..BENCHMARK_ITERATIONS {
        build_ovp_frame_optimized(&mut buffer, targets, payload);
        std::hint::black_box(&buffer);
    }
    let new_build_time = start.elapsed();

    // Test frame for parsing
    let test_frame = build_ovp_frame_old(targets, payload);

    // OLD VERSION - Frame parsing
    let start = Instant::now();
    for _ in 0..BENCHMARK_ITERATIONS {
        let result = parse_ovp_frame_old(&test_frame, test_drone_id);
        std::hint::black_box(result);
    }
    let old_parse_time = start.elapsed();

    // NEW VERSION - Frame parsing
    let start = Instant::now();
    for _ in 0..BENCHMARK_ITERATIONS {
        let result = parse_ovp_frame_fast(&test_frame, test_drone_id);
        std::hint::black_box(result);
    }
    let new_parse_time = start.elapsed();

    // Results
    print_comparison("Frame Building", old_build_time, new_build_time);
    print_comparison("Frame Parsing", old_parse_time, new_parse_time);

    // Memory allocation test
    benchmark_memory_usage(targets, payload);
}

fn benchmark_memory_usage(targets: &[u64], payload: &[u8]) {
    println!("\n🧠 Memory Allocation Benchmark");
    println!("─────────────────────────────────────");

    // OLD VERSION - Allocates Vec for each frame
    let start = Instant::now();
    let mut total_allocated = 0;
    for _ in 0..BENCHMARK_ITERATIONS {
        let frame = build_ovp_frame_old(targets, payload);
        total_allocated += frame.len();
        std::hint::black_box(frame);
    }
    let old_alloc_time = start.elapsed();

    // NEW VERSION - Reuses buffer
    let start = Instant::now();
    let mut buffer = vec![0u8; 1500];
    for _ in 0..BENCHMARK_ITERATIONS {
        build_ovp_frame_optimized(&mut buffer, targets, payload);
        std::hint::black_box(&buffer);
    }
    let new_alloc_time = start.elapsed();

    println!(
        "Old Version: {} allocations, {} total bytes",
        BENCHMARK_ITERATIONS, total_allocated
    );
    println!("New Version: 1 allocation, {} bytes", buffer.len());
    print_comparison("Memory Efficiency", old_alloc_time, new_alloc_time);
}

fn print_comparison(operation: &str, old_time: Duration, new_time: Duration) {
    let old_ns = old_time.as_nanos() as f64;
    let new_ns = new_time.as_nanos() as f64;
    let speedup = old_ns / new_ns;
    let improvement = ((old_ns - new_ns) / old_ns) * 100.0;

    println!("{}: ", operation);
    println!(
        "  Old: {:.2}ms ({:.0} ns/op)",
        old_time.as_secs_f64() * 1000.0,
        old_ns / BENCHMARK_ITERATIONS as f64
    );
    println!(
        "  New: {:.2}ms ({:.0} ns/op)",
        new_time.as_secs_f64() * 1000.0,
        new_ns / BENCHMARK_ITERATIONS as f64
    );
    println!("  📈 Speedup: {:.2}x ({:.1}% faster)", speedup, improvement);
    println!();
}

// OLD VERSION IMPLEMENTATIONS (copy from your original code)
fn build_ovp_frame_old(targets: &[u64], payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();

    // Header
    frame.extend_from_slice(&0xDEADBEEFu32.to_le_bytes());
    frame.extend_from_slice(&(targets.len() as u32).to_le_bytes());
    frame.extend_from_slice(&(payload.len() as u32).to_le_bytes());

    // Targets
    for &target in targets {
        frame.extend_from_slice(&target.to_le_bytes());
    }

    // Payload
    frame.extend_from_slice(payload);

    frame
}

fn parse_ovp_frame_old(frame: &[u8], my_id: u64) -> Option<Vec<u8>> {
    if frame.len() < 12 {
        return None;
    }

    // Check magic
    let magic = u32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]);
    if magic != 0xDEADBEEF {
        return None;
    }

    let target_count = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as usize;
    let payload_len = u32::from_le_bytes([frame[8], frame[9], frame[10], frame[11]]) as usize;

    let targets_start = 12;
    let targets_end = targets_start + (target_count * 8);
    let payload_start = targets_end;
    let payload_end = payload_start + payload_len;

    if frame.len() < payload_end {
        return None;
    }

    // Check if message is for me
    if target_count > 0 {
        let mut found = false;
        for i in 0..target_count {
            let offset = targets_start + (i * 8);
            let target_id = u64::from_le_bytes([
                frame[offset],
                frame[offset + 1],
                frame[offset + 2],
                frame[offset + 3],
                frame[offset + 4],
                frame[offset + 5],
                frame[offset + 6],
                frame[offset + 7],
            ]);
            if target_id == my_id {
                found = true;
                break;
            }
        }
        if !found {
            return None;
        }
    }

    // Extract payload
    Some(frame[payload_start..payload_end].to_vec())
}

// NEW VERSION IMPLEMENTATIONS (optimized)
fn build_ovp_frame_optimized(buffer: &mut [u8], targets: &[u64], payload: &[u8]) -> usize {
    use std::ptr;

    let header_size = 12;
    let targets_size = targets.len() * 8;
    let total_size = header_size + targets_size + payload.len();

    unsafe {
        let buf = buffer.as_mut_ptr();

        // Write header directly
        ptr::write_unaligned(buf as *mut u32, 0xDEADBEEFu32.to_le());
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
    }

    total_size
}

fn parse_ovp_frame_fast(frame: &[u8], my_id: u64) -> Option<&[u8]> {
    use std::ptr;

    if frame.len() < 12 {
        return None;
    }

    unsafe {
        let magic = ptr::read_unaligned(frame.as_ptr() as *const u32);
        if u32::from_le(magic) != 0xDEADBEEF {
            return None;
        }

        let target_count =
            u32::from_le(ptr::read_unaligned(frame.as_ptr().add(4) as *const u32)) as usize;
        let payload_len =
            u32::from_le(ptr::read_unaligned(frame.as_ptr().add(8) as *const u32)) as usize;

        let targets_start = 12;
        let targets_end = targets_start + (target_count * 8);
        let payload_start = targets_end;
        let payload_end = payload_start + payload_len;

        if frame.len() < payload_end {
            return None;
        }

        // Fast path for broadcasts
        if target_count == 0 {
            return Some(&frame[payload_start..payload_end]);
        }

        // Fast target checking
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
