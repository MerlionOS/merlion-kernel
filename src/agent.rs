/// OS-level Agent framework (Phase G).
/// Agents are autonomous kernel services that monitor, react, and
/// manage different aspects of the system.

use alloc::boxed::Box;
use alloc::vec::Vec;
use alloc::string::String;
use alloc::borrow::ToOwned;
use spin::Mutex;

const MAX_AGENTS: usize = 8;

static AGENTS: Mutex<Vec<AgentEntry>> = Mutex::new(Vec::new());

/// Trait for kernel agents.
pub trait Agent: Send + Sync {
    /// Agent name.
    fn name(&self) -> &str;
    /// One-line description.
    fn description(&self) -> &str;
    /// Called periodically by the agent scheduler.
    fn tick(&self);
    /// Called when the agent receives a message.
    fn handle_message(&self, msg: &str) -> String;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentState {
    Running,
    Paused,
}

struct AgentEntry {
    agent: Box<dyn Agent>,
    state: AgentState,
    tick_count: u64,
}

/// Agent info for display.
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub state: AgentState,
    pub ticks: u64,
}

/// Register an agent.
pub fn register(agent: Box<dyn Agent>) -> Result<(), &'static str> {
    let mut agents = AGENTS.lock();
    if agents.len() >= MAX_AGENTS {
        return Err("max agents reached");
    }
    crate::klog_println!("[agent] registered '{}'", agent.name());
    agents.push(AgentEntry {
        agent,
        state: AgentState::Running,
        tick_count: 0,
    });
    Ok(())
}

/// Tick all running agents (called periodically).
pub fn tick_all() {
    let mut agents = AGENTS.lock();
    for entry in agents.iter_mut() {
        if entry.state == AgentState::Running {
            entry.agent.tick();
            entry.tick_count += 1;
        }
    }
}

/// Send a message to a named agent.
pub fn send_message(name: &str, msg: &str) -> Option<String> {
    let agents = AGENTS.lock();
    for entry in agents.iter() {
        if entry.agent.name() == name && entry.state == AgentState::Running {
            return Some(entry.agent.handle_message(msg));
        }
    }
    None
}

/// Pause an agent.
pub fn pause(name: &str) -> Result<(), &'static str> {
    let mut agents = AGENTS.lock();
    for entry in agents.iter_mut() {
        if entry.agent.name() == name {
            entry.state = AgentState::Paused;
            return Ok(());
        }
    }
    Err("agent not found")
}

/// Resume an agent.
pub fn resume(name: &str) -> Result<(), &'static str> {
    let mut agents = AGENTS.lock();
    for entry in agents.iter_mut() {
        if entry.agent.name() == name {
            entry.state = AgentState::Running;
            return Ok(());
        }
    }
    Err("agent not found")
}

/// List all agents.
pub fn list() -> Vec<AgentInfo> {
    let agents = AGENTS.lock();
    agents.iter().map(|e| AgentInfo {
        name: e.agent.name().to_owned(),
        description: e.agent.description().to_owned(),
        state: e.state,
        ticks: e.tick_count,
    }).collect()
}

// ─── Built-in Agents ─────────────────────────────────

/// Health agent: monitors system health periodically.
struct HealthAgent;
impl Agent for HealthAgent {
    fn name(&self) -> &str { "health" }
    fn description(&self) -> &str { "System health monitor" }
    fn tick(&self) {
        // Silently check — alerts only on query
    }
    fn handle_message(&self, _msg: &str) -> String {
        let alerts = crate::ai_monitor::check();
        crate::ai_monitor::format_alerts(&alerts)
    }
}

/// Greeter agent: responds to conversational messages.
struct GreeterAgent;
impl Agent for GreeterAgent {
    fn name(&self) -> &str { "greeter" }
    fn description(&self) -> &str { "Conversational AI assistant" }
    fn tick(&self) {}
    fn handle_message(&self, msg: &str) -> String {
        crate::ai_syscall::infer(msg)
    }
}

/// Explain agent: explains kernel concepts.
struct ExplainAgent;
impl Agent for ExplainAgent {
    fn name(&self) -> &str { "explain" }
    fn description(&self) -> &str { "Kernel concept explainer" }
    fn tick(&self) {}
    fn handle_message(&self, msg: &str) -> String {
        crate::ai_syscall::explain(msg)
    }
}

/// Register built-in agents.
pub fn init() {
    let _ = register(Box::new(HealthAgent));
    let _ = register(Box::new(GreeterAgent));
    let _ = register(Box::new(ExplainAgent));
}
