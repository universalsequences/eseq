use super::*;

pub(super) struct GraphEditBatchGuard {
    lg: *mut crate::audiograph::LiveGraph,
    pub(super) serial: u64,
}

impl GraphEditBatchGuard {
    pub(super) fn new(lg: *mut crate::audiograph::LiveGraph) -> Self {
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        let serial = unsafe { crate::audiograph::graph_edit_current_batch_serial(lg) };
        debug_assert!(serial > 0);
        Self { lg, serial }
    }
}

impl Drop for GraphEditBatchGuard {
    fn drop(&mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
    }
}

pub(super) unsafe fn disconnect_all_ports(lg: *mut crate::audiograph::LiveGraph, src_id: i32, dst_id: i32) {
    for src_port in 0..2 {
        for dst_port in 0..2 {
            crate::audiograph::graph_disconnect(lg, src_id, src_port, dst_id, dst_port);
        }
    }
}

pub(super) unsafe fn connect_stereo_pair(lg: *mut crate::audiograph::LiveGraph, src_id: i32, dst_id: i32) {
    crate::audiograph::graph_connect(lg, src_id, 0, dst_id, 0);
    crate::audiograph::graph_connect(lg, src_id, 1, dst_id, 1);
}

pub(super) fn add_gain_node_checked(
    lg: *mut crate::audiograph::LiveGraph,
    gain: f32,
    name: &str,
    context: &str,
) -> Result<i32, String> {
    let c_name = CString::new(name).map_err(|_| format!("{context}: node name contains NUL"))?;
    let node_id = unsafe { crate::audiograph::add_gain_node(lg, gain, c_name.as_ptr()) };
    if node_id < 0 {
        return Err(format!("{context}: failed to queue gain node '{name}'"));
    }
    Ok(node_id)
}

pub(super) struct GraphNodeBuildTransaction {
    lg: *mut crate::audiograph::LiveGraph,
    node_ids: Vec<i32>,
    connections: Vec<(i32, i32, i32, i32)>,
    max_nodes: usize,
    max_connections: usize,
    finished: bool,
}

impl GraphNodeBuildTransaction {
    pub(super) fn new(
        lg: *mut crate::audiograph::LiveGraph,
        max_nodes: usize,
        max_connections: usize,
    ) -> Result<Self, String> {
        let required_edits = max_nodes
            .checked_add(max_connections)
            .and_then(|forward_edits| forward_edits.checked_mul(2))
            .ok_or_else(|| "Graph edit transaction capacity overflow".to_string())?;
        unsafe { crate::audiograph::begin_graph_edit_batch(lg) };
        // GraphEditQueue is single-producer. Reserving room for both the
        // complete build and its inverse commands makes Drop rollback
        // infallible without rewinding a queue the audio thread may be reading.
        let available_edits = unsafe { crate::audiograph::graph_edit_queue_available(lg) } as usize;
        if available_edits < required_edits {
            unsafe { crate::audiograph::end_graph_edit_batch(lg) };
            return Err(format!(
                "Graph edit queue has room for {available_edits} commands; route construction requires {required_edits} for build and rollback"
            ));
        }
        Ok(Self {
            lg,
            node_ids: Vec::with_capacity(max_nodes),
            connections: Vec::with_capacity(max_connections),
            max_nodes,
            max_connections,
            finished: false,
        })
    }

    pub(super) fn own(&mut self, node_id: i32) -> Result<i32, String> {
        self.node_ids.push(node_id);
        #[cfg(test)]
        record_test_graph_build_node(node_id);
        if self.node_ids.len() > self.max_nodes {
            return Err(format!(
                "Graph edit transaction created more than {} reserved nodes",
                self.max_nodes
            ));
        }
        Ok(node_id)
    }

    pub(super) fn connect(
        &mut self,
        src_node: i32,
        src_port: i32,
        dst_node: i32,
        dst_port: i32,
        context: &str,
    ) -> Result<(), String> {
        if self.connections.len() >= self.max_connections {
            return Err(format!(
                "{context}: graph edit transaction exceeded its {} reserved connections",
                self.max_connections
            ));
        }
        let connected = unsafe {
            crate::audiograph::graph_connect(self.lg, src_node, src_port, dst_node, dst_port)
        };
        if !connected {
            return Err(format!(
                "{context}: graph_connect({src_node}, {src_port}, {dst_node}, {dst_port}) failed"
            ));
        }
        self.connections
            .push((src_node, src_port, dst_node, dst_port));
        #[cfg(test)]
        record_test_graph_build_connection((src_node, src_port, dst_node, dst_port));
        Ok(())
    }

    pub(super) fn commit(mut self) {
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
        self.finished = true;
    }
}

impl Drop for GraphNodeBuildTransaction {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        let mut rollback_succeeded = true;
        for &(src_node, src_port, dst_node, dst_port) in self.connections.iter().rev() {
            let queued = unsafe {
                crate::audiograph::graph_disconnect(self.lg, src_node, src_port, dst_node, dst_port)
            };
            rollback_succeeded &= queued;
            #[cfg(test)]
            if queued {
                record_test_graph_build_rollback_connection((
                    src_node, src_port, dst_node, dst_port,
                ));
            }
        }
        for &node_id in self.node_ids.iter().rev() {
            let queued = unsafe { crate::audiograph::delete_node(self.lg, node_id) };
            rollback_succeeded &= queued;
            #[cfg(test)]
            if queued {
                record_test_graph_build_rollback_node(node_id);
            }
        }
        unsafe { crate::audiograph::end_graph_edit_batch(self.lg) };
        self.finished = true;
        if !rollback_succeeded {
            eprintln!(
                "Graph edit rollback could not enqueue every inverse command despite its capacity reservation"
            );
        }
    }
}

pub(super) fn add_engine_route_gain_node_checked(
    lg: *mut crate::audiograph::LiveGraph,
    gain: f32,
    name: &str,
    context: &str,
) -> Result<i32, String> {
    check_test_graph_build_node_add(context)?;
    add_gain_node_checked(lg, gain, name, context)
}

pub(super) fn check_test_graph_build_node_add(context: &str) -> Result<(), String> {
    #[cfg(test)]
    if should_fail_test_graph_build_node_add() {
        return Err(format!("{context}: injected graph node allocation failure"));
    }
    Ok(())
}

#[cfg(test)]
thread_local! {
    static TEST_GRAPH_BUILD_FAIL_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
    static TEST_GRAPH_BUILD_NODE_IDS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS: std::cell::RefCell<Vec<i32>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_CONNECTIONS: std::cell::RefCell<Vec<(i32, i32, i32, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
    static TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS: std::cell::RefCell<Vec<(i32, i32, i32, i32)>> = const { std::cell::RefCell::new(Vec::new()) };
}

#[cfg(test)]
pub(super) fn begin_test_graph_build_capture() {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| ids.borrow_mut().clear());
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| ids.borrow_mut().clear());
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| connections.borrow_mut().clear());
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS.with(|connections| connections.borrow_mut().clear());
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| remaining.set(None));
}

#[cfg(test)]
pub(super) fn set_test_graph_build_failure_after(successful_adds: usize) {
    begin_test_graph_build_capture();
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| remaining.set(Some(successful_adds)));
}

#[cfg(test)]
pub(super) fn should_fail_test_graph_build_node_add() -> bool {
    TEST_GRAPH_BUILD_FAIL_AFTER.with(|remaining| match remaining.get() {
        Some(0) => {
            remaining.set(None);
            true
        }
        Some(count) => {
            remaining.set(Some(count - 1));
            false
        }
        None => false,
    })
}

#[cfg(test)]
pub(super) fn record_test_graph_build_node(node_id: i32) {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| ids.borrow_mut().push(node_id));
}

#[cfg(test)]
pub(super) fn record_test_graph_build_rollback_node(node_id: i32) {
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| ids.borrow_mut().push(node_id));
}

#[cfg(test)]
pub(super) fn record_test_graph_build_connection(connection: (i32, i32, i32, i32)) {
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| connections.borrow_mut().push(connection));
}

#[cfg(test)]
pub(super) fn record_test_graph_build_rollback_connection(connection: (i32, i32, i32, i32)) {
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS
        .with(|connections| connections.borrow_mut().push(connection));
}

#[cfg(test)]
pub(super) fn take_test_graph_build_node_ids() -> Vec<i32> {
    TEST_GRAPH_BUILD_NODE_IDS.with(|ids| std::mem::take(&mut *ids.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_test_graph_build_rollback_node_ids() -> Vec<i32> {
    TEST_GRAPH_BUILD_ROLLBACK_NODE_IDS.with(|ids| std::mem::take(&mut *ids.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_test_graph_build_connections() -> Vec<(i32, i32, i32, i32)> {
    TEST_GRAPH_BUILD_CONNECTIONS.with(|connections| std::mem::take(&mut *connections.borrow_mut()))
}

#[cfg(test)]
pub(super) fn take_test_graph_build_rollback_connections() -> Vec<(i32, i32, i32, i32)> {
    TEST_GRAPH_BUILD_ROLLBACK_CONNECTIONS
        .with(|connections| std::mem::take(&mut *connections.borrow_mut()))
}
