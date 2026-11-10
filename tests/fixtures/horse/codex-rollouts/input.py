import json,sys
version=sys.argv[1]
id='01900000-0000-7000-8000-000000000001'
stamp='2026-01-01T00:00:00Z'
usage=dict(input_tokens=10,cached_input_tokens=2,output_tokens=3,reasoning_output_tokens=1,total_tokens=13)
rows=[
('session_meta',dict(id=id,session_id=id,timestamp=stamp,cwd='/work',originator='cli',cli_version=version,source='cli',model_provider='openai',base_instructions=None,forked_from_id='01900000-0000-7000-8000-000000000002')),
('dynamic_tools_updated',dict(thread_id=id,tools=[])),
('response_item',dict(type='message',role='user',content=[dict(type='input_text',text='Hello'),dict(type='input_audio',audio_url='data:audio/wav;base64,AA==')])),
('inter_agent_communication',dict(author='/root',recipient='/root/worker',content='hello',trigger_turn=False)),
('inter_agent_communication_metadata',dict(trigger_turn=False)),
('compacted',dict(message='summary',replacement_history=[])),
('turn_context',dict(cwd='/work',approval_policy='never',sandbox_policy=dict(type='danger-full-access'),model='model')),
('token_usage_record',dict(thread_id=id,turn_id='turn',session_id=id,root_turn_id='turn',response_id='response',usage=usage,turn_token_usage=usage,thread_token_usage=usage)),
('world_state',dict(full=True,state=dict(ratio=0.125))),
('security_risk_score',dict(scores=dict(risk=0.125))),
('retained_context',dict(type='verified_answer',turn_id='turn',call_id='call',questions=[])),
('event_msg',dict(type='context_compacted')),
('realtime_item',dict(id='item',realtime_session_id='session',type='realtime_session_started')),
('event_msg',dict(type='task_started',turn_id='replay-turn',model_context_window=None)),
('response_item',dict(type='message',role='user',content=[dict(type='input_text',text='Replay question')])),
('event_msg',dict(type='user_message',message='Replay question')),
('response_item',dict(type='message',role='assistant',content=[dict(type='output_text',text='Replay answer')],phase='final_answer')),
('event_msg',dict(type='agent_message',message='Replay answer',phase='final_answer')),
('event_msg',dict(type='task_complete',turn_id='replay-turn',last_agent_message='Replay answer')),
]
if version=='0.167.0': rows.insert(3,('response_item',dict(type='message',role='user',content=[dict(type='input_audio',file_id='file-1')])))
for i,(kind,payload) in enumerate(rows): print(json.dumps(dict(timestamp=stamp,ordinal=i,type=kind,payload=payload),separators=(',',':')))
