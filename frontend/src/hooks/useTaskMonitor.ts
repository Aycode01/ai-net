import { useState, useEffect, useCallback } from 'react';
import type { TaskResponse, DAGEvent, PaymentEvent } from '../types/api';
import { apiClient } from '../services/api';
import { useTaskWebSocket } from './useTaskWebSocket';
import { useNodeState } from './useNodeState';
import { useTaskPayments } from './useTaskPayments';
import { useTaskOutputs } from './useTaskOutputs';

export const useTaskMonitor = (taskId: string | undefined) => {
  const [task, setTask] = useState<TaskResponse | null>(null);
  const [loading, setLoading] = useState<boolean>(true);
  const [error, setError] = useState<Error | null>(null);

  // Initialize sub-hooks
  const nodeState = useNodeState(taskId || '');
  const paymentState = useTaskPayments(taskId || '');
  const outputState = useTaskOutputs(taskId || '');

  // Handle WebSocket events
  const handleWebSocketMessage = useCallback((event: DAGEvent) => {
    // Update node state
    nodeState.updateNodeFromEvent(event);
    
    // Update payment state
    paymentState.updatePaymentFromEvent(event);
    
    // Update output state
    outputState.updateOutputFromEvent(event);
    
    // Update task state for global events
    if (event.type === 'task_completed') {
      setTask(prev => prev ? { ...prev, status: 'completed' } : null);
    } else if (event.type === 'task_failed') {
      setTask(prev => prev ? { ...prev, status: 'failed' } : null);
    }
  }, [nodeState, paymentState, outputState]);

  // WebSocket connection
  const { isConnected, status: wsStatus } = useTaskWebSocket({
    taskId: taskId || '',
    onMessage: handleWebSocketMessage,
  });

  const fetchTask = async (id: string) => {
    try {
      setLoading(true);
      const data = await apiClient.get<TaskResponse>(`/api/tasks/${id}`);
      setTask(data);
      if (data.dag) {
        // Initialize all sub-hooks with fetched data
        nodeState.initializeNodes(data.dag);
        
        const completedNodes = data.dag.filter(node => node.status === 'completed');

        // Only include payments that have actual amount and transaction data.
        // Do not fabricate placeholders like 'mock-hash' or estimated amounts.
        const initialPayments: PaymentEvent[] = data.dag
          .filter(node => {
            const hasAmount = (node.result as any)?.amount;
            const hasTxHash = (node.result as any)?.txHash;
            return hasAmount && hasTxHash;
          })
          .map(node => ({
            amount: (node.result as any).amount,
            direction: 'out' as const,
            counterparty: node.agentType || 'agent',
            memo: `Payment released for ${node.nodeId}`,
            timestamp: (node.result as any).timestamp || node.updatedAt || data.updatedAt,
            txHash: (node.result as any).txHash,
          }));

        outputState.initializeOutputs(completedNodes);
        paymentState.initializePayments(initialPayments);
      }
      setError(null);
    } catch (err: any) {
      console.error('Failed to fetch task details:', err);
      setError(err);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    if (!taskId) return;
    
    fetchTask(taskId);
  }, [taskId]);

  return {
    task,
    loading,
    error,
    wsStatus,
    nodes: nodeState.nodes,
    payments: paymentState.payments,
    outputs: outputState.outputs,
    finalResult: outputState.finalResult,
    isConnected,
    refetch: () => taskId && fetchTask(taskId),
  };
};
