"""Shared wire assertions for the independent workspace boundary fixtures."""
import json


def assert_tool_batch(chat, calls, rejected=False, pure_errors=()):
    if rejected and len(calls) > 1:
        # An unknown failure stops dispatch. Keep every rejection boundary
        # covered by admitting each as a separate explicit request.
        return [assert_tool_batch(chat, [call], True, pure_errors)[0] for call in calls]
    needs_review = rejected and calls[0][0] not in pure_errors
    code, body, count = chat(calls)
    expected_status = 'requireshumaninput' if needs_review else 'completed'
    assert code == 200 and body['status'] == expected_status, (code, body.get('status'), count)
    records = body['tool_calls']
    assert len(records) == len(calls) and count == (1 if needs_review else 2), (len(records), len(calls), count)
    outputs = []
    for (name, _args), record in zip(calls, records):
        assert record['tool_name'] == name
        result = record['result']
        if rejected:
            assert isinstance(result, dict) and result.get('error'), (name, result)
            assert result['effect_status'] == ('unknown' if needs_review else 'no_effect'), (name, result)
        else:
            assert isinstance(result, str), (name, result)
            result = json.loads(result)
            assert isinstance(result, dict)
        outputs.append(result)
    return outputs
