//! SOCKS operations over the canonical Rust proxy owner and its packet buffer.
use crate::{
    SocksTransition, swr_v1_socks_auth, swr_v1_socks_connect, swr_v1_socks_negotiate,
    swr_v1_socks_transition,
};
use std::ptr;
use swoole_rs_corebase::{ProxyState, net_types::AddressStorage};

#[repr(C)]
pub struct ProxyPacketResult {
    pub size: i64,
    pub diagnostic: i32,
    pub last_error: i32,
}
/// Pack negotiate(0), authentication(1), or connection(2) using owned fields.
/// # Safety
/// owner is a live SOCKS owner, exclusively lent through packet/state writes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_owned_packet(
    owner: *mut ProxyState,
    action: u32,
) -> ProxyPacketResult {
    // SAFETY: caller lends the live stable owner.
    let owner = unsafe { &*owner };
    // SAFETY: a scalar snapshot before accessing its distinct packet storage.
    let fields = unsafe { *owner.fields.get() };
    // SAFETY: SOCKS owners contain an exclusive512-byte packet allocation.
    let buffer = unsafe { owner.buffer() };
    let mut result = ProxyPacketResult {
        size: -1,
        diagnostic: 0,
        last_error: 0,
    };
    match action {
        0 => {
            // SAFETY: the stable username slot is unchanged while packing.
            let user = unsafe { owner.string(1) };
            // SAFETY: the owner's method field is exclusively lent to negotiation.
            let method = unsafe { ptr::addr_of_mut!((*owner.fields.get()).method) };
            // SAFETY: method and packet bytes are disjoint, stable writable slots.
            result.size =
                unsafe { swr_v1_socks_negotiate(u8::from(user.length != 0), method, buffer, 512) };
        }
        1 => {
            // SAFETY: the owner's credentials are stable and disjoint from packet storage.
            let user = unsafe { owner.string(1) };
            // SAFETY: the password slot remains unchanged while packing.
            let password = unsafe { owner.string(2) };
            // SAFETY: stable credential spans and exclusive512-byte packet output.
            result.size = unsafe {
                swr_v1_socks_auth(
                    user.data.cast(),
                    user.length,
                    password.data.cast(),
                    password.length,
                    buffer,
                    512,
                )
            };
        }
        2 => {
            // Preserve the initialized prefix even when literal address validation fails.
            // SAFETY: the owner exclusively lends at least three packet bytes.
            unsafe { ptr::copy_nonoverlapping([5u8, 1, 0].as_ptr(), buffer, 3) };
            // SAFETY: the target slot stays stable through address parsing and packing.
            let host = unsafe { owner.string(3) };
            // SAFETY: every member of this POD sockaddr union admits zero bytes.
            let mut address: AddressStorage = unsafe { std::mem::zeroed() };
            let mut length = 0;
            let (kind, bytes, size) = if fields.dns_tunnel != 0 {
                (3, host.data.cast::<u8>(), host.length)
            } else {
                // SAFETY: the exact address operation receives a local union/length and terminated host.
                let error = unsafe {
                    swoole_rs_clients::address::swr_v1_clients_address_assign(
                        ptr::from_mut(&mut address).cast(),
                        &raw mut length,
                        fields.socket_type,
                        host.data,
                        host.length,
                        fields.target_port,
                        None,
                    )
                };
                if error != 0 {
                    result.diagnostic = 1;
                    result.last_error = error;
                    return result;
                }
                if swoole_rs_reactor::type_is(fields.socket_type, swoole_rs_reactor::CLASS_INET4) {
                    // SAFETY: the preceding address operation initialized the IPv4 union arm.
                    let bytes = unsafe { ptr::addr_of!(address.inet_v4.sin_addr).cast::<u8>() };
                    (1, bytes, size_of::<libc::in_addr>())
                } else if swoole_rs_reactor::type_is(
                    fields.socket_type,
                    swoole_rs_reactor::CLASS_INET6,
                ) {
                    // SAFETY: the preceding address operation initialized the IPv6 union arm.
                    let bytes = unsafe { ptr::addr_of!(address.inet_v6.sin6_addr).cast::<u8>() };
                    (4, bytes, size_of::<libc::in6_addr>())
                } else {
                    result.diagnostic = 2;
                    return result;
                }
            };
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "native SOCKS packet API narrows its int target port to uint16_t"
            )]
            let port = fields.target_port as u16;
            // SAFETY: the selected initialized address is disjoint from the owned packet buffer.
            result.size = unsafe { swr_v1_socks_connect(kind, bytes, size, port, buffer, 512) };
            if result.size == -2 {
                result.size = -1;
                result.diagnostic = 3;
            }
        }
        _ => {}
    }
    result
}
/// Apply a response transition to the proxy before native diagnostics or sends.
/// # Safety
/// owner is exclusive; data lends length readable bytes; output is writable/disjoint.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn swr_v1_socks_owned_transition(
    owner: *mut ProxyState,
    data: *const u8,
    length: usize,
    output: *mut SocksTransition,
) -> i32 {
    // SAFETY: caller lends a live stable owner.
    let owner = unsafe { &*owner };
    // SAFETY: leaf scalar snapshot before response parsing.
    let fields = unsafe { *owner.fields.get() };
    let mut transition = SocksTransition::default();
    // SAFETY: data and the local transition have the existing codec's disjoint input/output contract.
    let status = unsafe {
        swr_v1_socks_transition(
            fields.state,
            fields.method,
            data,
            length,
            &raw mut transition,
        )
    };
    if status != 0 {
        return status;
    }
    if transition.error == 0 {
        // SAFETY: state advances before packing/sending, matching native member order.
        unsafe { (*owner.fields.get()).state = transition.state };
    }
    // SAFETY: caller lends the disjoint writable transition output.
    unsafe { output.write(transition) };
    0
}
