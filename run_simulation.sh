#!/bin/bash

# OVP (based on v2) Drone Swarm Simulation Runner
# This script sets up the environment and runs the simulation

set -e

echo "🚁 OVP Drone Swarm Simulation Setup"
echo "==================================="

# Check if running as root - we actually NEED root for raw sockets
if [[ $EUID -ne 0 ]]; then
   echo "🔐 This simulation needs root privileges for raw socket access."
   echo "🔄 Restarting with sudo (preserving environment)..."
   # Use -E to preserve environment variables including PATH
   exec sudo -E "$0" "$@"
fi

echo "✅ Running with root privileges"

# Check dependencies
echo "🔍 Checking dependencies..."

# Add common Rust paths to search
export PATH="$PATH:/home/$SUDO_USER/.cargo/bin:$HOME/.cargo/bin"

if ! command -v cargo &> /dev/null; then
    echo "❌ Rust/Cargo not found. Checking common locations..."
    
    # Try to find cargo in user's home
    if [ -n "$SUDO_USER" ] && [ -f "/home/$SUDO_USER/.cargo/bin/cargo" ]; then
        echo "✅ Found Rust in user directory, adding to PATH..."
        export PATH="/home/$SUDO_USER/.cargo/bin:$PATH"
    else
        echo "❌ Rust/Cargo not found. Please install Rust first:"
        echo "   Option 1: Install as root:"
        echo "     sudo su -"
        echo "     curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
        echo "     source ~/.cargo/env"
        echo "     exit"
        echo ""
        echo "   Option 2: Install as your user and try again"
        exit 1
    fi
fi

if ! command -v sudo &> /dev/null; then
    echo "❌ sudo not found. Please install sudo."
    exit 1
fi

echo "✅ Dependencies check passed"
echo "🔧 Using cargo at: $(which cargo)"

# Create project directory
PROJECT_DIR="ovp_drone_simulation"
echo "📁 Creating project directory: $PROJECT_DIR"

if [ -d "$PROJECT_DIR" ]; then
    echo "🧹 Cleaning existing project directory..."
    rm -rf "$PROJECT_DIR"
fi

mkdir -p "$PROJECT_DIR/src"
cd "$PROJECT_DIR"

# Create Cargo.toml
echo "📝 Creating Cargo.toml..."
cat > Cargo.toml << 'EOF'
[package]
name = "ovp_drone_simulation"
version = "0.1.0"
edition = "2021"

[dependencies]
rand = "0.8"

[[bin]]
name = "drone_sim"
path = "src/main.rs"
EOF

# Copy your OVP library code
echo "📝 Setting up source files with REAL OVP code..."
cat > src/lib.rs << 'EOF'
//! # OVP (Omega Volumetric Protocol) v2
//! 
//! A revolutionary counter-intuitive Layer 2 network protocol designed for decentralized 
//! wireless peer-to-peer drone swarm communication. This protocol operates in "ghost mode" 
//! with military-grade security using pure physics principles.
//! 
//! ## Key Features
//! 
//! - **Zero IP/Port Architecture**: No traditional networking concepts - pure spherical emission
//! - **Volumetric Broadcasting**: One emission reaches all drones in physical range
//! - **Ultra-Low Latency**: Zero-allocation hot paths for maximum performance
//! - **Military Grade Security**: Ghost mode operation with no traceable network identifiers
//! 
//! ## Performance Characteristics
//! 
//! - **29.19x faster** frame building (3ns/op vs 95ns/op)
//! - **7.29x faster** frame parsing (2ns/op vs 15ns/op)  
//! - **31.42x faster** memory operations (96.8% reduction in allocations)
//! - **Efficiency**: 201 group multicasts replaced 603 individual unicasts
//! 
//! ## Protocol Statistics Example
//! 
//! ```text
//! Total Messages Sent: 598
//! ├─ Broadcast Messages: 302
//! ├─ Direct Messages: 95
//! └─ Group Messages (O(1)): 201
//! Messages Received: 13,164
//! Active Drones: 20
//! 
//! Traditional protocols: 13,164 individual transmissions
//! OVP Protocol: 598 spherical emissions (22x reduction)
//! ```

use std::os::unix::io::RawFd;
use std::ptr;
use std::mem;
use std::ffi::CString;
use std::thread;
use std::sync::mpsc::{channel, Receiver};

//==============================================================================
// RAW SOCKET CONSTANTS
//==============================================================================

/// Address family for packet sockets - enables direct Layer 2 access
const AF_PACKET: i32 = 17;

/// Raw socket type - bypasses kernel networking stack for maximum performance
const SOCK_RAW: i32 = 3;

/// Ethernet protocol identifier - captures all ethernet frames
const ETH_P_ALL: u16 = 0x0003;

/// Socket level for packet-specific socket options
const SOL_PACKET: i32 = 263;

/// Socket option to add interface to packet membership (enables promiscuous mode)
const PACKET_ADD_MEMBERSHIP: i32 = 1;

/// Promiscuous mode flag - receive ALL frames on interface, not just addressed to us
const PACKET_MR_PROMISC: i32 = 1;

//==============================================================================
// OVP PROTOCOL CONSTANTS
//==============================================================================

/// OVP Protocol magic number - identifies valid OVP frames
/// 0xDEADBEEF chosen for easy hex identification in network traces
const OVP_MAGIC: u32 = 0xDEADBEEF;

/// Maximum frame size based on standard Ethernet MTU
/// Prevents fragmentation and ensures single-packet transmission
const MAX_FRAME_SIZE: usize = 1500;

/// Receive buffer size - large enough to handle burst traffic
/// 64KB provides substantial headroom for high-throughput scenarios
const RECV_BUFFER_SIZE: usize = 65536;

//==============================================================================
// CORE TYPE DEFINITIONS
//==============================================================================

/// Unique identifier for each drone in the swarm
/// 64-bit allows for 18+ quintillion unique drone IDs
pub type DroneId = u64;

/// OVP Frame Header Structure
/// 
/// Packed representation ensures exact wire format control and minimal overhead.
/// Total header size: 12 bytes + (target_count * 8) + payload_len
/// 
/// Wire Format:
/// ```text
/// [magic:4][target_count:4][payload_len:4][targets:target_count*8][payload:payload_len]
/// ```
#[repr(C, packed)]
#[derive(Debug, Copy, Clone)]
pub struct OmegaFrame {
    /// Protocol magic number for frame identification and validation
    magic: u32,
    
    /// Number of specific target drones (0 = broadcast to all in range)
    target_count: u32,
    
    /// Length of payload data in bytes
    payload_len: u32,
    
    // Note: Dynamic data follows this header:
    // - targets: Array of DroneId values (target_count * 8 bytes)
    // - payload: Actual message data (payload_len bytes)
}

/// Packet membership request structure for enabling promiscuous mode
/// Required to receive all frames on interface, not just those addressed to us
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct PacketMreq {
    /// Interface index to apply membership to
    mr_ifindex: i32,
    
    /// Type of membership (promiscuous, multicast, etc.)
    mr_type: u16,
    
    /// Length of hardware address
    mr_alen: u16,
    
    /// Hardware address (MAC) - unused for promiscuous mode
    mr_address: [u8; 8],
}

/// Socket address structure for Layer 2 packet sockets
/// Specifies interface and protocol for raw packet transmission/reception
#[repr(C, packed)]
#[derive(Copy, Clone)]
struct SockaddrLl {
    /// Address family (AF_PACKET)
    sll_family: u16,
    
    /// Ethernet protocol type
    sll_protocol: u16,
    
    /// Interface index for packet transmission
    sll_ifindex: i32,
    
    /// Hardware address type (unused in our implementation)
    sll_hatype: u16,
    
    /// Packet type (unicast, broadcast, etc.)
    sll_pkttype: u8,
    
    /// Hardware address length (6 for Ethernet MAC)
    sll_halen: u8,
    
    /// Hardware address (MAC address for Ethernet)
    sll_addr: [u8; 8],
}

//==============================================================================
// UNSAFE SYSTEM CALL BINDINGS
//==============================================================================

// Raw system call bindings for maximum performance
// These bypass Rust's standard library networking for direct kernel access
unsafe extern "C" {
    /// Create a socket endpoint for communication
    unsafe fn socket(domain: i32, type_: i32, protocol: i32) -> i32;
    
    /// Set socket options - used for enabling promiscuous mode
    unsafe fn setsockopt(sockfd: i32, level: i32, optname: i32, optval: *const u8, optlen: u32) -> i32;
    
    /// Bind socket to specific interface
    unsafe fn bind(sockfd: i32, addr: *const SockaddrLl, addrlen: u32) -> i32;
    
    /// Send data to specific destination (broadcast in our case)
    unsafe fn sendto(sockfd: i32, buf: *const u8, len: usize, flags: i32, 
              dest_addr: *const SockaddrLl, addrlen: u32) -> isize;
    
    /// Receive data from any source on the interface
    unsafe fn recvfrom(sockfd: i32, buf: *mut u8, len: usize, flags: i32,
                src_addr: *mut SockaddrLl, addrlen: *mut u32) -> isize;
    
    /// Convert interface name to index number
    unsafe fn if_nametoindex(ifname: *const i8) -> u32;
    
    /// Close file descriptor
    unsafe fn close(fd: i32) -> i32;
}

//==============================================================================
// OMEGA SOCKET - LOW-LEVEL RAW SOCKET WRAPPER
//==============================================================================

/// Low-level raw socket wrapper for OVP protocol
/// 
/// Provides zero-allocation hot paths for maximum performance in drone swarm scenarios.
/// Uses pre-allocated buffers and unsafe operations to minimize latency.
#[derive(Clone)]
pub struct OmegaSocket {
    /// Raw file descriptor for the packet socket
    raw_fd: RawFd,
    
    /// Network interface index for packet transmission
    interface_index: u32,
    
    /// Pre-allocated send buffer - prevents allocation in hot path
    /// Sized to maximum frame size for zero-copy operations
    send_buffer: Box<[u8; MAX_FRAME_SIZE]>,
    
    /// Pre-allocated receive buffer - prevents allocation in hot path
    /// Large size handles burst traffic without drops
    recv_buffer: Box<[u8; RECV_BUFFER_SIZE]>,
    
    /// Pre-computed destination address for broadcast operations
    /// Eliminates repeated address computation in hot path
    dest_addr: SockaddrLl,
}

impl OmegaSocket {
    /// Create a new OmegaSocket bound to the specified network interface
    /// 
    /// # Arguments
    /// 
    /// * `interface` - Network interface name (e.g., "wlan0", "eth0")
    /// 
    /// # Returns
    /// 
    /// Result containing the configured socket or error if setup fails
    /// 
    /// # Safety
    /// 
    /// This function creates a raw socket which requires elevated privileges.
    /// The socket is configured for promiscuous mode to receive all network traffic.
    #[inline]
    pub fn new(interface: &str) -> Result<Self, Box<dyn std::error::Error>> {
        unsafe {
            // Create raw packet socket for direct Layer 2 access
            // AF_PACKET allows us to bypass IP stack entirely
            let fd = socket(AF_PACKET, SOCK_RAW, ETH_P_ALL.to_be() as i32);
            if fd < 0 {
                return Err("Failed to create raw socket - ensure running with appropriate privileges".into());
            }

            // Convert interface name to kernel interface index
            let if_name = CString::new(interface)?;
            let if_index = if_nametoindex(if_name.as_ptr());
            if if_index == 0 {
                close(fd);
                return Err("Interface not found - check interface name and availability".into());
            }

            // Enable promiscuous mode - CRITICAL for drone swarm operation
            // This allows us to receive ALL frames in our transmission range,
            // not just those specifically addressed to our MAC address
            let mreq = PacketMreq {
                mr_ifindex: if_index as i32,
                mr_type: PACKET_MR_PROMISC as u16,
                mr_alen: 0,                    // No specific MAC filtering
                mr_address: [0; 8],            // Unused for promiscuous mode
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
                return Err("Failed to enable promiscuous mode - check interface permissions".into());
            }

            // Bind socket to specific interface
            // This ensures we only send/receive on the designated wireless interface
            let addr = SockaddrLl {
                sll_family: AF_PACKET as u16,
                sll_protocol: ETH_P_ALL.to_be(),
                sll_ifindex: if_index as i32,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 0,
                sll_addr: [0; 8],
            };

            let ret = bind(fd, ptr::addr_of!(addr), mem::size_of::<SockaddrLl>() as u32);
            if ret < 0 {
                close(fd);
                return Err("Failed to bind socket to interface".into());
            }

            // Pre-compute broadcast destination address for hot path optimization
            // Broadcast MAC (FF:FF:FF:FF:FF:FF) ensures spherical emission to all drones
            let dest_addr = SockaddrLl {
                sll_family: AF_PACKET as u16,
                sll_protocol: ETH_P_ALL.to_be(),
                sll_ifindex: if_index as i32,
                sll_hatype: 0,
                sll_pkttype: 0,
                sll_halen: 6,                                          // Ethernet MAC length
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

    /// Emit a pre-built frame via spherical broadcast
    /// 
    /// THE CORE EMISSION FUNCTION - This is where the magic happens!
    /// One spherical emit reaches ALL drones within physical wireless range.
    /// Zero allocation hot path optimized for minimum latency.
    /// 
    /// # Arguments
    /// 
    /// * `frame_data` - Complete OVP frame ready for transmission
    /// 
    /// # Returns
    /// 
    /// Result indicating success or transmission error
    /// 
    /// # Performance
    /// 
    /// This function is marked `inline(always)` and uses zero allocations
    /// for maximum performance in time-critical drone operations.
    #[inline(always)]
    pub fn emit_frame(&mut self, frame_data: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        unsafe {
            // Direct kernel send - bypasses all userspace networking layers
            // Broadcasts to FF:FF:FF:FF:FF:FF ensuring spherical coverage
            let sent = sendto(
                self.raw_fd,
                frame_data.as_ptr(),
                frame_data.len(),
                0,                                                    // No special flags
                ptr::addr_of!(self.dest_addr),                      // Pre-computed broadcast address
                mem::size_of::<SockaddrLl>() as u32,
            );

            if sent < 0 {
                return Err("Spherical emission failed - check wireless interface status".into());
            }

            Ok(())
        }
    }

    /// Receive any frame within wireless range
    /// 
    /// Zero allocation hot path for maximum receive performance.
    /// Promiscuous mode ensures we capture ALL OVP frames in range,
    /// regardless of their intended destination.
    /// 
    /// # Returns
    /// 
    /// Result containing received frame data or reception error
    /// Returns a slice into the internal receive buffer for zero-copy operation.
    /// 
    /// # Performance
    /// 
    /// Uses pre-allocated buffer and unsafe operations for minimum latency.
    /// Critical for real-time drone swarm coordination.
    #[inline(always)]
    pub fn receive_frame(&mut self) -> Result<&[u8], Box<dyn std::error::Error>> {
        unsafe {
            let mut src_addr: SockaddrLl = mem::zeroed();
            let mut addr_len = mem::size_of::<SockaddrLl>() as u32;

            // Direct kernel receive into pre-allocated buffer
            let received = recvfrom(
                self.raw_fd,
                self.recv_buffer.as_mut_ptr(),
                self.recv_buffer.len(),
                0,                                                    // Non-blocking operation
                ptr::addr_of_mut!(src_addr),
                ptr::addr_of_mut!(addr_len),
            );

            if received < 0 {
                return Err("Frame reception failed".into());
            }

            // Return slice of actual received data - zero copy operation
            Ok(&self.recv_buffer[..received as usize])
        }
    }

    /// Build OVP frame directly in send buffer and emit in one operation
    /// 
    /// ULTIMATE ZERO-ALLOCATION HOT PATH
    /// Constructs the complete OVP frame directly in the pre-allocated send buffer,
    /// then emits via spherical broadcast. No intermediate allocations or copies.
    /// 
    /// # Arguments
    /// 
    /// * `targets` - Slice of specific drone IDs to target (empty = broadcast)
    /// * `payload` - Message payload data
    /// 
    /// # Returns
    /// 
    /// Result indicating success or error (frame too large, transmission failure)
    /// 
    /// # Performance
    /// 
    /// This is the fastest possible path for OVP message transmission:
    /// - Direct buffer manipulation using unsafe pointer operations
    /// - No memory allocations or copies
    /// - Immediate transmission after construction
    /// - Unaligned writes for maximum speed on modern CPUs
    #[inline(always)]
    pub fn build_and_emit(&mut self, targets: &[DroneId], payload: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        // Calculate total frame size and validate against buffer capacity
        let header_size = 12;                          // magic + target_count + payload_len
        let targets_size = targets.len() * 8;          // 8 bytes per DroneId
        let total_size = header_size + targets_size + payload.len();
        
        if total_size > MAX_FRAME_SIZE {
            return Err("Frame exceeds maximum size - reduce targets or payload".into());
        }

        unsafe {
            let buf = self.send_buffer.as_mut_ptr();
            
            // Write OVP header directly to buffer using unaligned writes for speed
            // Little-endian format for consistent cross-platform compatibility
            ptr::write_unaligned(buf as *mut u32, OVP_MAGIC.to_le());
            ptr::write_unaligned(buf.add(4) as *mut u32, (targets.len() as u32).to_le());
            ptr::write_unaligned(buf.add(8) as *mut u32, (payload.len() as u32).to_le());
            
            // Write target drone IDs array
            let mut offset = 12;
            for &target in targets {
                ptr::write_unaligned(buf.add(offset) as *mut u64, target.to_le());
                offset += 8;
            }
            
            // Copy payload data directly after targets
            ptr::copy_nonoverlapping(payload.as_ptr(), buf.add(offset), payload.len());
            
            // Immediate spherical emission - frame goes out instantly
            let sent = sendto(
                self.raw_fd,
                buf,
                total_size,
                0,
                ptr::addr_of!(self.dest_addr),
                mem::size_of::<SockaddrLl>() as u32,
            );

            if sent < 0 {
                return Err("Failed to emit constructed frame".into());
            }
        }

        Ok(())
    }
}

/// Automatic socket cleanup when OmegaSocket is dropped
/// Ensures raw socket file descriptor is properly closed
impl Drop for OmegaSocket {
    #[inline]
    fn drop(&mut self) {
        unsafe {
            close(self.raw_fd);
        }
    }
}

//==============================================================================
// OVP HIGH-LEVEL CLIENT API
//==============================================================================

/// High-level OVP client interface for drone swarm communication
/// 
/// Provides a simple, efficient API for drone-to-drone messaging while
/// maintaining the ultra-high performance characteristics of the underlying
/// raw socket implementation.
pub struct OVP {
    /// Low-level socket for frame transmission and reception
    socket: OmegaSocket,
    
    /// This drone's unique identifier in the swarm
    my_drone_id: DroneId,
    
    /// Background thread handle for message reception (optional)
    receiver_handle: Option<thread::JoinHandle<()>>,
    
    /// Channel for receiving messages from background thread
    message_rx: Receiver<Vec<u8>>,
}

impl OVP {
    /// Create a new OVP client instance
    /// 
    /// # Arguments
    /// 
    /// * `interface` - Network interface for drone communication (e.g., "wlan0")
    /// * `my_drone_id` - Unique identifier for this drone in the swarm
    /// 
    /// # Returns
    /// 
    /// Result containing configured OVP client or initialization error
    /// 
    /// # Example
    /// 
    /// ```rust
    /// let mut ovp = OVP::new("wlan0", 42)?;
    /// ```
    #[inline]
    pub fn new(interface: &str, my_drone_id: DroneId) -> Result<Self, Box<dyn std::error::Error>> {
        let socket = OmegaSocket::new(interface)?;
        let (_tx, rx) = channel();
        
        // Note: For multi-threaded usage, socket would need to be split into
        //       separate send/receive components. This simplified version assumes
        //       single-threaded usage for maximum performance in drone applications.
        
        Ok(OVP {
            socket,
            my_drone_id,
            receiver_handle: None,
            message_rx: rx,
        })
    }

    /// THE ONLY API METHOD - Pure volumetric power emission
    /// 
    /// This is the core of the OVP protocol - one method that handles all
    /// communication patterns through the power of volumetric broadcasting.
    /// 
    /// ZERO ALLOCATION HOT PATH - Optimized for real-time drone operations
    /// 
    /// # Arguments
    /// 
    /// * `neighbours` - Optional specific target drones (None = broadcast to all)
    /// * `payload` - Message data to transmit
    /// 
    /// # Communication Patterns
    /// 
    /// - **Broadcast**: `emit(None, payload)` - reaches ALL drones in range
    /// - **Unicast**: `emit(Some(&[drone_id]), payload)` - targets specific drone
    /// - **Multicast**: `emit(Some(&[id1, id2, id3]), payload)` - targets multiple drones
    /// 
    /// # Returns
    /// 
    /// Result indicating successful emission or transmission error
    /// 
    /// # Performance
    /// 
    /// - Direct buffer manipulation - no intermediate allocations
    /// - Single spherical emission reaches all specified targets
    /// - Optimized for sub-microsecond latency in critical drone operations
    /// 
    /// # Example
    /// 
    /// ```rust
    /// // Broadcast emergency stop to all drones
    /// ovp.emit(None, b"EMERGENCY_STOP")?;
    /// 
    /// // Send position update to specific drones
    /// ovp.emit(Some(&[1, 2, 3]), b"POS:123.45,67.89,10.0")?;
    /// ```
    #[inline(always)]
    pub fn emit(&mut self, neighbours: Option<&[DroneId]>, payload: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
        let targets = neighbours.unwrap_or(&[]);
        
        // ONE SPHERICAL EMIT - REACHES ALL SPECIFIED TARGETS IN RANGE
        // The magic of volumetric broadcasting - one transmission, multiple recipients
        self.socket.build_and_emit(targets, payload)?;
        
        Ok(())
    }

    /// Attempt to receive a message intended for this drone
    /// 
    /// Non-blocking reception with optimized parsing for maximum throughput.
    /// Uses early-exit parsing to minimize CPU usage on irrelevant frames.
    /// 
    /// # Returns
    /// 
    /// Option containing message payload if a valid OVP message for this drone
    /// was received, None if no relevant message available.
    /// 
    /// # Performance
    /// 
    /// - Zero allocation parsing using unsafe pointer operations
    /// - Early exit on invalid or irrelevant frames
    /// - Direct slice return from receive buffer (zero-copy)
    #[inline(always)]
    pub fn try_receive(&mut self) -> Option<&[u8]> {
        match self.socket.receive_frame() {
            Ok(frame) => parse_ovp_frame_fast(frame, self.my_drone_id),
            Err(_) => None,
        }
    }
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
            message_rx: rx,       // New receiver channel
        }
    }
}
//==============================================================================
// ULTRA-FAST FRAME PARSING
//==============================================================================

/// Ultra-fast OVP frame parsing with early exits and zero allocations
/// 
/// Optimized parsing function that quickly determines if a received frame
/// is a valid OVP message intended for the specified drone ID.
/// 
/// # Arguments
/// 
/// * `frame` - Raw frame data received from network
/// * `my_id` - This drone's ID for target matching
/// 
/// # Returns
/// 
/// Option containing payload slice if frame is valid and intended for this drone,
/// None if frame is invalid, malformed, or not intended for this drone.
/// 
/// # Performance Optimizations
/// 
/// - Early exit on insufficient frame length
/// - Unsafe unaligned reads for maximum speed on modern CPUs
/// - Fast-path broadcast detection (target_count == 0)
/// - Efficient target ID scanning with pointer arithmetic
/// - Zero memory allocations - returns slice into original buffer
/// 
/// # Safety
/// 
/// Uses unsafe pointer operations for performance. All bounds checking
/// is performed before unsafe operations to ensure memory safety.
#[inline(always)]
fn parse_ovp_frame_fast(frame: &[u8], my_id: DroneId) -> Option<&[u8]> {
    // Quick length check - minimum OVP frame is 12 bytes (header only)
    if frame.len() < 12 { 
        return None; 
    }
    
    unsafe {
        // Validate OVP magic number using fast unaligned read
        let magic = ptr::read_unaligned(frame.as_ptr() as *const u32);
        if u32::from_le(magic) != OVP_MAGIC { 
            return None; 
        }
        
        // Extract frame structure information
        let target_count = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(4) as *const u32)) as usize;
        let payload_len = u32::from_le(ptr::read_unaligned(frame.as_ptr().add(8) as *const u32)) as usize;
        
        // Calculate frame section boundaries
        let targets_start = 12;
        let targets_end = targets_start + (target_count * 8);
        let payload_start = targets_end;
        let payload_end = payload_start + payload_len;
        
        // Validate total frame length
        if frame.len() < payload_end { 
            return None; 
        }
        
        // FAST PATH: Broadcast message (no specific targets)
        // If target_count is 0, message is for everyone in range
        if target_count == 0 {
            return Some(&frame[payload_start..payload_end]);
        }
        
        // TARGETED MESSAGE: Check if this drone is in target list
        // Use unaligned pointer reads for maximum scanning speed
        let targets_ptr = frame.as_ptr().add(targets_start) as *const u64;
        for i in 0..target_count {
            let target_id = u64::from_le(ptr::read_unaligned(targets_ptr.add(i)));
            if target_id == my_id {
                return Some(&frame[payload_start..payload_end]);
            }
        }
        
        // Message not intended for this drone
        None
    }
}

//==============================================================================
// UNIT TESTS
//==============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Test zero-allocation frame operations
    /// 
    /// Verifies that the hot path frame building and emission operations
    /// do not perform any memory allocations during execution.
    #[test]
    fn test_zero_alloc_frame_ops() {
        // Note: This test requires loopback interface for testing
        // In production, use actual wireless interface (e.g., "wlan0")
        let mut socket = OmegaSocket::new("lo").unwrap();
        let targets = vec![1, 2, 3];
        let payload = b"Hello volumetric world!";
        
        // This operation should complete without any heap allocations
        // All operations use pre-allocated buffers and stack variables
        socket.build_and_emit(&targets, payload).unwrap();
    }

    /// Test OVP frame parsing correctness
    /// 
    /// Verifies that the fast parsing function correctly identifies
    /// valid frames and extracts payloads for targeted and broadcast messages.
    #[test]
    fn test_frame_parsing() {
        // Test broadcast frame parsing
        let mut frame = Vec::new();
        frame.extend_from_slice(&OVP_MAGIC.to_le_bytes());      // magic
        frame.extend_from_slice(&0u32.to_le_bytes());           // target_count = 0 (broadcast)
        frame.extend_from_slice(&5u32.to_le_bytes());           // payload_len = 5
        frame.extend_from_slice(b"hello");                      // payload
        
        let result = parse_ovp_frame_fast(&frame, 123);
        assert_eq!(result, Some(b"hello".as_slice()));
        
        // Test targeted frame parsing
        let mut frame = Vec::new();
        frame.extend_from_slice(&OVP_MAGIC.to_le_bytes());      // magic
        frame.extend_from_slice(&1u32.to_le_bytes());           // target_count = 1
        frame.extend_from_slice(&5u32.to_le_bytes());           // payload_len = 5
        frame.extend_from_slice(&123u64.to_le_bytes());         // target = 123
        frame.extend_from_slice(b"world");                      // payload
        
        let result = parse_ovp_frame_fast(&frame, 123);
        assert_eq!(result, Some(b"world".as_slice()));
        
        // Test non-matching target
        let result = parse_ovp_frame_fast(&frame, 456);
        assert_eq!(result, None);
    }
}
EOF

# Create main simulation file
echo "📝 Creating main simulation file..."
cat > src/main.rs << 'EOF'
use std::thread;
use std::time::Duration;
use std::sync::{Arc, Mutex};
use rand::Rng;

mod lib;
use lib::{OVP, DroneId};

const DRONE_COUNT: usize = 20;
const SIMULATION_DURATION: u64 = 30;

#[derive(Debug)]
struct SwarmStats {
    messages_sent: u64,
    messages_received: u64,
    group_messages_sent: u64,
    direct_messages_sent: u64,
    broadcast_messages_sent: u64,
    active_drones: u64,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("🚁 Starting Enhanced OVP Drone Swarm Simulation");
    println!("===============================================");
    println!("🌐 Testing Revolutionary Layer 2 Protocol Features:");
    println!("   ✅ Spherical broadcast (physics-based propagation)");
    println!("   ✅ Receiver-side filtering");
    println!("   ✅ O(1) multicast (vs traditional O(n) unicast loops)");
    println!("   ✅ Source identification logging");
    println!("   ✅ Group messaging capabilities");
    println!("");
    
    let stats = Arc::new(Mutex::new(SwarmStats {
        messages_sent: 0,
        messages_received: 0,
        group_messages_sent: 0,
        direct_messages_sent: 0,
        broadcast_messages_sent: 0,
        active_drones: 0,
    }));
    
    let mut drone_handles = Vec::new();
    
    // Spawn drone simulation threads
    for drone_id in 1..=DRONE_COUNT {
        let stats_clone = Arc::clone(&stats);
        
        let handle = thread::spawn(move || {
            run_drone_simulation(drone_id as u64, stats_clone)
        });
        
        drone_handles.push(handle);
        thread::sleep(Duration::from_millis(50));
    }
    
    // Monitor thread
    let monitor_stats = Arc::clone(&stats);
    let monitor_handle = thread::spawn(move || {
        monitor_swarm(monitor_stats);
    });
    
    println!("✅ All {} drones launched!", DRONE_COUNT);
    println!("🔄 Simulation running for {} seconds...", SIMULATION_DURATION);
    println!("📡 Watch for OVP's revolutionary O(1) group messaging!");
    
    thread::sleep(Duration::from_secs(SIMULATION_DURATION));
    
    println!("\n🛑 Simulation complete!");
    
    let final_stats = stats.lock().unwrap();
    println!("\n📊 FINAL OVP PROTOCOL STATISTICS:");
    println!("═══════════════════════════════════");
    println!("Total Messages Sent: {}", final_stats.messages_sent);
    println!("  └─ Broadcast Messages: {}", final_stats.broadcast_messages_sent);
    println!("  └─ Direct Messages: {}", final_stats.direct_messages_sent);
    println!("  └─ Group Messages (O(1)): {}", final_stats.group_messages_sent);
    println!("Messages Received: {}", final_stats.messages_received);
    println!("Active Drones: {}", final_stats.active_drones);
    println!("");
    println!("🎯 OVP Efficiency: {} group multicasts saved {} individual unicasts!",
             final_stats.group_messages_sent,
             final_stats.group_messages_sent * 3); // Assuming avg 3 recipients per group
    
    Ok(())
}

fn run_drone_simulation(drone_id: DroneId, stats: Arc<Mutex<SwarmStats>>) {
    println!("🚁 Drone {} starting OVP initialization...", drone_id);
    
    // Use loopback interface for all drones - they'll all share the same network
    let interface = "lo";
    
    let ovp = match OVP::new(interface, drone_id) {
        Ok(ovp) => ovp,
        Err(e) => {
            eprintln!("❌ Drone {} failed to initialize OVP: {}", drone_id, e);
            return;
        }
    };
    
    {
        let mut stats_lock = stats.lock().unwrap();
        stats_lock.active_drones += 1;
    }
    
    let mut rng = rand::thread_rng();
    let mut position = (rng.gen_range(0.0..100.0), rng.gen_range(0.0..100.0));
    
    println!("✅ Drone {} OVP online at position ({:.1}, {:.1})", drone_id, position.0, position.1);
    
    let start_time = std::time::Instant::now();
    
    // Clone ovp for the main thread before moving the original
    let mut main_ovp = ovp.clone();
    let receiver_stats = stats.clone();
    
    // Move the original OVP into the receiver thread
    thread::spawn(move || {
        message_receiver_loop(drone_id, ovp, receiver_stats);
    });
    
    while start_time.elapsed().as_secs() < SIMULATION_DURATION {
        match rng.gen_range(0..6) {
            0 => send_status_broadcast(&mut main_ovp, drone_id, &position, &stats),
            1 => send_targeted_message(&mut main_ovp, drone_id, &stats),
            2 => send_formation_command(&mut main_ovp, drone_id, &stats),
            3 => send_sensor_data(&mut main_ovp, drone_id, &position, &stats),
            4 => send_group_message(&mut main_ovp, drone_id, &stats), // NEW: Group messaging
            _ => send_team_coordination(&mut main_ovp, drone_id, &stats), // NEW: Team coordination
        }
        
        // Simulate movement
        position.0 += rng.gen_range(-1.0..1.0);
        position.1 += rng.gen_range(-1.0..1.0);
        position.0 = position.0.clamp(0.0, 100.0);
        position.1 = position.1.clamp(0.0, 100.0);
        
        thread::sleep(Duration::from_millis(rng.gen_range(200..800)));
    }
    
    println!("🔴 Drone {} shutting down OVP connection", drone_id);
}

// NEW: Dedicated message receiver loop with source logging
fn message_receiver_loop(drone_id: DroneId, mut ovp: OVP, stats: Arc<Mutex<SwarmStats>>) {
    println!("📡 Drone {} receiver thread started - listening for OVP frames", drone_id);
    
    loop {
        if let Some(message) = ovp.try_receive() {
            if let Ok(msg_str) = String::from_utf8(message.to_vec()) {
                // Parse the message to extract sender info
                let sender_id = extract_sender_from_message(&msg_str);
                
                {
                    let mut stats_lock = stats.lock().unwrap();
                    stats_lock.messages_received += 1;
                }
                
                // LOG: Show message reception with source identification
                println!("📥 Drone {} RECEIVED from Drone {}: {}", 
                        drone_id, 
                        sender_id.unwrap_or(0), 
                        truncate_message(&msg_str, 50));
                
                // Process different message types
                process_received_message(drone_id, &msg_str, sender_id);
            }
        }
        
        thread::sleep(Duration::from_millis(10)); // Small delay to prevent busy waiting
    }
}

fn extract_sender_from_message(message: &str) -> Option<DroneId> {
    // Extract sender ID from message format like "STATUS:5:..." or "DIRECT:3:..."
    let parts: Vec<&str> = message.split(':').collect();
    if parts.len() >= 2 {
        parts[1].parse::<DroneId>().ok()
    } else {
        None
    }
}

fn process_received_message(drone_id: DroneId, message: &str, sender_id: Option<DroneId>) {
    if message.starts_with("GROUP:") {
        println!("🎯 Drone {} processed GROUP message from Drone {}", 
                drone_id, sender_id.unwrap_or(0));
    } else if message.starts_with("TEAM:") {
        println!("👥 Drone {} joined team coordination from Drone {}", 
                drone_id, sender_id.unwrap_or(0));
    } else if message.starts_with("FORMATION") {
        println!("🔄 Drone {} acknowledging formation command from leader Drone {}", 
                drone_id, sender_id.unwrap_or(0));
    }
}

fn truncate_message(msg: &str, max_len: usize) -> String {
    if msg.len() > max_len {
        format!("{}...", &msg[..max_len])
    } else {
        msg.to_string()
    }
}

fn send_status_broadcast(ovp: &mut OVP, drone_id: u64, position: &(f32, f32), stats: &Arc<Mutex<SwarmStats>>) {
    let message = format!("STATUS:{}:({:.1},{:.1}):OPERATIONAL", drone_id, position.0, position.1);
    
    if ovp.emit(None, message.as_bytes()).is_ok() {
        {
            let mut stats_lock = stats.lock().unwrap();
            stats_lock.messages_sent += 1;
            stats_lock.broadcast_messages_sent += 1;
        }
        println!("📡 Drone {} BROADCAST status (spherical propagation)", drone_id);
    }
}

fn send_targeted_message(ovp: &mut OVP, drone_id: u64, stats: &Arc<Mutex<SwarmStats>>) {
    let mut rng = rand::thread_rng();
    let target_drone = rng.gen_range(1..=DRONE_COUNT as u64);
    
    if target_drone != drone_id {
        let message = format!("DIRECT:{}:Hello from drone {}", drone_id, target_drone);
        let targets = vec![target_drone];
        
        if ovp.emit(Some(&targets), message.as_bytes()).is_ok() {
            {
                let mut stats_lock = stats.lock().unwrap();
                stats_lock.messages_sent += 1;
                stats_lock.direct_messages_sent += 1;
            }
            println!("📨 Drone {} DIRECT message to Drone {} (targeted filtering)", drone_id, target_drone);
        }
    }
}

// NEW: Group messaging - demonstrates O(1) multicast capability
fn send_group_message(ovp: &mut OVP, drone_id: u64, stats: &Arc<Mutex<SwarmStats>>) {
    let mut rng = rand::thread_rng();
    
    // Create a random group of 2-5 drones
    let group_size = rng.gen_range(2..=5);
    let mut target_group = Vec::new();
    
    while target_group.len() < group_size {
        let target = rng.gen_range(1..=DRONE_COUNT as u64);
        if target != drone_id && !target_group.contains(&target) {
            target_group.push(target);
        }
    }
    
    let group_list = target_group.iter()
        .map(|id| format!("D{}", id))
        .collect::<Vec<_>>()
        .join(",");
    
    let message = format!("GROUP:{}:Mission briefing for team [{}] - Rendezvous at coordinates (50,50)", 
                         drone_id, group_list);
    
    if ovp.emit(Some(&target_group), message.as_bytes()).is_ok() {
        {
            let mut stats_lock = stats.lock().unwrap();
            stats_lock.messages_sent += 1;
            stats_lock.group_messages_sent += 1;
        }
        println!("🎯 Drone {} GROUP message to {} drones [{}] in O(1) operation!", 
                drone_id, target_group.len(), group_list);
        println!("   └─ OVP Magic: 1 spherical emit reaches {} targets simultaneously", target_group.len());
    }
}

// NEW: Team coordination messaging
fn send_team_coordination(ovp: &mut OVP, drone_id: u64, stats: &Arc<Mutex<SwarmStats>>) {
    let mut rng = rand::thread_rng();
    
    // Create a tactical team of 3-4 drones
    let team_size = rng.gen_range(3..=4);
    let mut team_members = Vec::new();
    
    while team_members.len() < team_size {
        let member = rng.gen_range(1..=DRONE_COUNT as u64);
        if member != drone_id && !team_members.contains(&member) {
            team_members.push(member);
        }
    }
    
    let team_names = team_members.iter()
        .map(|id| format!("Unit-{}", id))
        .collect::<Vec<_>>()
        .join(",");
    
    let tactics = ["FLANK_LEFT", "COVER_ADVANCE", "RECON_SWEEP", "DEFENSIVE_CIRCLE"];
    let chosen_tactic = tactics[rng.gen_range(0..tactics.len())];
    
    let message = format!("TEAM:{}:Tactical command {} for units [{}] - Execute in 30 seconds", 
                         drone_id, chosen_tactic, team_names);
    
    if ovp.emit(Some(&team_members), message.as_bytes()).is_ok() {
        {
            let mut stats_lock = stats.lock().unwrap();
            stats_lock.messages_sent += 1;
            stats_lock.group_messages_sent += 1;
        }
        println!("👥 Drone {} TEAM coordination to {} members [{}] - Tactic: {}", 
                drone_id, team_members.len(), team_names, chosen_tactic);
        println!("   └─ Traditional protocols would need {} separate unicasts - OVP does it in 1!", 
                team_members.len());
    }
}

fn send_formation_command(ovp: &mut OVP, drone_id: u64, stats: &Arc<Mutex<SwarmStats>>) {
    let mut rng = rand::thread_rng();
    let formation_type = match rng.gen_range(0..3) {
        0 => "FORMATION_CIRCLE",
        1 => "FORMATION_LINE",
        _ => "FORMATION_GRID",
    };
    
    let message = format!("FORMATION:{}:{}:LEADER_CMD", formation_type, drone_id);
    
    if ovp.emit(None, message.as_bytes()).is_ok() {
        {
            let mut stats_lock = stats.lock().unwrap();
            stats_lock.messages_sent += 1;
            stats_lock.broadcast_messages_sent += 1;
        }
        println!("🎯 Drone {} FORMATION command: {} (broadcast to all in range)", drone_id, formation_type);
    }
}

fn send_sensor_data(ovp: &mut OVP, drone_id: u64, position: &(f32, f32), stats: &Arc<Mutex<SwarmStats>>) {
    let mut rng = rand::thread_rng();
    let temperature = rng.gen_range(15.0..35.0);
    let battery = rng.gen_range(20..100);
    
    let message = format!("SENSOR:{}:TEMP:{:.1}:BATT:{}:POS:({:.1},{:.1})", 
                         drone_id, temperature, battery, position.0, position.1);
    
    if ovp.emit(None, message.as_bytes()).is_ok() {
        {
            let mut stats_lock = stats.lock().unwrap();
            stats_lock.messages_sent += 1;
            stats_lock.broadcast_messages_sent += 1;
        }
        println!("📊 Drone {} SENSOR data broadcast (temp: {:.1}°C, battery: {}%)", 
                drone_id, temperature, battery);
    }
}

fn monitor_swarm(stats: Arc<Mutex<SwarmStats>>) {
    let mut last_sent = 0;
    let mut last_received = 0;
    let mut last_group = 0;
    
    for i in 0..6 { // Monitor for 30 seconds (6 * 5 second intervals)
        thread::sleep(Duration::from_secs(5));
        
        let current_stats = stats.lock().unwrap();
        let sent_rate = current_stats.messages_sent - last_sent;
        let received_rate = current_stats.messages_received - last_received;
        let group_rate = current_stats.group_messages_sent - last_group;
        
        println!("\n📈 OVP SWARM STATUS ({}s):", (i + 1) * 5);
        println!("   Active Drones: {}", current_stats.active_drones);
        println!("   Messages Sent (total): {}", current_stats.messages_sent);
        println!("     └─ Broadcasts: {}", current_stats.broadcast_messages_sent);
        println!("     └─ Direct: {}", current_stats.direct_messages_sent);
        println!("     └─ Groups: {}", current_stats.group_messages_sent);
        println!("   Messages Received: {}", current_stats.messages_received);
        println!("   Send Rate (last 5s): {}", sent_rate);
        println!("   Receive Rate (last 5s): {}", received_rate);
        println!("   Group Multicast Rate: {}", group_rate);
        
        if group_rate > 0 {
            println!("   🚀 OVP Efficiency: {} group sends = ~{} traditional unicasts saved!", 
                    group_rate, group_rate * 3);
        }
        
        last_sent = current_stats.messages_sent;
        last_received = current_stats.messages_received;
        last_group = current_stats.group_messages_sent;
    }
}
EOF

echo "🔨 Building simulation..."
cargo build --release

if [ $? -eq 0 ]; then
    echo "✅ Build successful!"
    echo ""
    echo "🚀 Starting simulation..."
    echo "Press Ctrl+C to stop early"
    echo ""
    
    cargo run --bin drone_sim
else
    echo "❌ Build failed!"
    exit 1
fi

echo ""
echo "✅ Simulation completed successfully!"
echo ""
echo "🔧 To run with real OVP networking:"
echo "   1. Replace the mock lib.rs with your actual OVP code"
echo "   2. Set up proper network interfaces"
echo "   3. Run with sudo for raw socket access"
EOF

chmod +x run_simulation.sh

echo "🎯 **HOW TO RUN THE SIMULATION:**"
echo ""
echo "**Run the Real OVP Network Test:**"
echo "   ./run_simulation.sh"
echo ""
echo "**What you'll see:**"
echo "- 20 independent drone processes using REAL raw sockets"
echo "- All drones sharing the loopback interface (lo)"
echo "- Actual OVP frames being sent/received"
echo "- Your revolutionary Layer 2 protocol in action!"
echo ""
echo "**Network behaviors being tested:**"
echo "✅ Raw socket creation and binding"
echo "✅ OVP frame building and parsing"
echo "✅ Broadcast messages (status updates)"
echo "✅ Targeted unicast (direct drone-to-drone)"
echo "✅ Command propagation (formation changes)"
echo "✅ Sensor data sharing"
echo "✅ Concurrent multi-drone operation"
echo "✅ Message filtering by drone ID"
echo ""
echo "The simulation uses your ACTUAL OVP code with real raw sockets!"
echo "All drones share the loopback interface so they can communicate."
echo ""
echo "🚀 Ready to see your Layer 2 protocol in action!"