//! The `TCP_INFO` fields that both the admin server's `get_socket_info` command and
//! `Coroutine\Socket::getOption` report, in upstream's order.
use core::ffi::CStr;

#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) type TcpInfo = libc::tcp_info;
#[cfg(any(target_os = "linux", target_os = "android"))]
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) const TCP_INFO: core::ffi::c_int = libc::TCP_INFO;
#[cfg(target_vendor = "apple")]
pub(crate) type TcpInfo = libc::tcp_connection_info;
#[cfg(target_vendor = "apple")]
#[cfg(any(test, feature = "upstream-symbols"))]
pub(crate) const TCP_INFO: core::ffi::c_int = libc::TCP_CONNECTION_INFO;

/// The fields upstream's `sw_socket_parse_tcp_info` reports, in its order.
#[cfg(any(target_os = "linux", target_os = "android"))]
#[must_use]
pub fn tcp_fields(t: &TcpInfo) -> [(&'static CStr, u64); 32] {
    let wscale = t.tcpi_snd_rcv_wscale;
    [
        (c"state", t.tcpi_state.into()),
        (c"ca_state", t.tcpi_ca_state.into()),
        (c"retransmits", t.tcpi_retransmits.into()),
        (c"probes", t.tcpi_probes.into()),
        (c"backoff", t.tcpi_backoff.into()),
        (c"options", t.tcpi_options.into()),
        // `tcpi_snd_wscale : 4, tcpi_rcv_wscale : 4`.
        (c"snd_wscale", (wscale & 0x0f).into()),
        (c"rcv_wscale", (wscale >> 4).into()),
        (c"rto", t.tcpi_rto.into()),
        (c"ato", t.tcpi_ato.into()),
        (c"snd_mss", t.tcpi_snd_mss.into()),
        (c"rcv_mss", t.tcpi_rcv_mss.into()),
        (c"unacked", t.tcpi_unacked.into()),
        (c"sacked", t.tcpi_sacked.into()),
        (c"lost", t.tcpi_lost.into()),
        (c"retrans", t.tcpi_retrans.into()),
        (c"fackets", t.tcpi_fackets.into()),
        (c"last_data_sent", t.tcpi_last_data_sent.into()),
        (c"last_ack_sent", t.tcpi_last_ack_sent.into()),
        (c"last_data_recv", t.tcpi_last_data_recv.into()),
        (c"last_ack_recv", t.tcpi_last_ack_recv.into()),
        (c"pmtu", t.tcpi_pmtu.into()),
        (c"rcv_ssthresh", t.tcpi_rcv_ssthresh.into()),
        (c"rtt", t.tcpi_rtt.into()),
        (c"rttvar", t.tcpi_rttvar.into()),
        (c"snd_ssthresh", t.tcpi_snd_ssthresh.into()),
        (c"snd_cwnd", t.tcpi_snd_cwnd.into()),
        (c"advmss", t.tcpi_advmss.into()),
        (c"reordering", t.tcpi_reordering.into()),
        (c"rcv_rtt", t.tcpi_rcv_rtt.into()),
        (c"rcv_space", t.tcpi_rcv_space.into()),
        (c"total_retrans", t.tcpi_total_retrans.into()),
    ]
}

#[cfg(target_vendor = "apple")]
#[must_use]
pub fn tcp_fields(t: &TcpInfo) -> [(&'static CStr, u64); 14] {
    [
        (c"state", t.tcpi_state.into()),
        (c"snd_wscale", t.tcpi_snd_wscale.into()),
        (c"rcv_wscale", t.tcpi_rcv_wscale.into()),
        (c"options", t.tcpi_options.into()),
        (c"flags", t.tcpi_flags.into()),
        (c"rto", t.tcpi_rto.into()),
        (c"maxseg", t.tcpi_maxseg.into()),
        (c"snd_ssthresh", t.tcpi_snd_ssthresh.into()),
        (c"snd_cwnd", t.tcpi_snd_cwnd.into()),
        (c"snd_wnd", t.tcpi_snd_wnd.into()),
        (c"snd_sbbytes", t.tcpi_snd_sbbytes.into()),
        (c"rcv_wnd", t.tcpi_rcv_wnd.into()),
        (c"srtt", t.tcpi_srtt.into()),
        (c"rttvar", t.tcpi_rttvar.into()),
    ]
}
