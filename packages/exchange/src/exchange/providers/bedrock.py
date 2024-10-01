import hashlib
import hmac
import json
import logging
import os
from datetime import datetime, timezone
from typing import Any, Dict, List, Optional, Tuple, Type
from urllib.parse import quote, urlparse

import httpx

from exchange.content import Text, ToolResult, ToolUse
from exchange.message import Message
from exchange.providers import Provider, Usage
from tenacity import retry, wait_fixed, stop_after_attempt
from exchange.providers.utils import retry_if_status
from exchange.providers.utils import raise_for_status
from exchange.tool import Tool

SERVICE = "bedrock-runtime"
UTC = timezone.utc

logger = logging.getLogger(__name__)

retry_procedure = retry(
    wait=wait_fixed(2),
    stop=stop_after_attempt(2),
    retry=retry_if_status(codes=[429], above=500),
    reraise=True,
)


class AwsClient(httpx.Client):
    def __init__(
        self,
        aws_region: str,
        aws_access_key: str,
        aws_secret_key: str,
        aws_session_token: Optional[str] = None,
        **kwargs: Dict[str, Any],
    ) -> None:
        self.region = aws_region
        self.host = f"https://{SERVICE}.{aws_region}.amazonaws.com/"
        self.access_key = aws_access_key
        self.secret_key = aws_secret_key
        self.session_token = aws_session_token
        super().__init__(base_url=self.host, timeout=600, **kwargs)

    def post(self, path: str, json: Dict, **kwargs: Dict[str, Any]) -> httpx.Response:
        signed_headers = self.sign_and_get_headers(
            method="POST",
            url=path,
            payload=json,
            service="bedrock",
        )
        return super().post(url=path, json=json, headers=signed_headers, **kwargs)

    def sign_and_get_headers(
        self,
        method: str,
        url: str,
        payload: dict,
        service: str,
    ) -> Dict[str, str]:
        """
        Sign the request and generate the necessary headers for AWS authentication.

        Args:
            method (str): HTTP method (e.g., 'GET', 'POST').
            url (str): The request URL.
            payload (dict): The request payload.
            service (str): The AWS service name.
            region (str): The AWS region.
            access_key (str): The AWS access key.
            secret_key (str): The AWS secret key.
            session_token (Optional[str]): The AWS session token, if any.

        Returns:
            Dict[str, str]: The headers required for the request.
        """

        def sign(key: bytes, msg: str) -> bytes:
            return hmac.new(key, msg.encode("utf-8"), hashlib.sha256).digest()

        def get_signature_key(key: str, date_stamp: str, region_name: str, service_name: str) -> bytes:
            k_date = sign(("AWS4" + key).encode("utf-8"), date_stamp)
            k_region = sign(k_date, region_name)
            k_service = sign(k_region, service_name)
            k_signing = sign(k_service, "aws4_request")
            return k_signing

        # Convert payload to JSON string
        request_parameters = json.dumps(payload)

        # Create a date for headers and the credential string
        t = datetime.now(UTC)
        amz_date = t.strftime("%Y%m%dT%H%M%SZ")
        date_stamp = t.strftime("%Y%m%d")  # Date w/o time, used in credential scope

        # Create canonical URI and headers
        parsedurl = urlparse(url)
        canonical_uri = quote(parsedurl.path if parsedurl.path else "/", safe="/-_.~")
        canonical_headers = f"host:{parsedurl.netloc}\nx-amz-date:{amz_date}\n"

        # Create the list of signed headers.
        signed_headers = "host;x-amz-date"
        if self.session_token:
            canonical_headers += "x-amz-security-token:" + self.session_token + "\n"
            signed_headers += ";x-amz-security-token"

        # Create payload hash
        payload_hash = hashlib.sha256(request_parameters.encode("utf-8")).hexdigest()

        # Canonical request
        canonical_request = f"{method}\n{canonical_uri}\n\n{canonical_headers}\n{signed_headers}\n{payload_hash}"

        # Create the string to sign
        algorithm = "AWS4-HMAC-SHA256"
        credential_scope = f"{date_stamp}/{self.region}/{service}/aws4_request"
        string_to_sign = (
            f"{algorithm}\n{amz_date}\n{credential_scope}\n"
            f'{hashlib.sha256(canonical_request.encode("utf-8")).hexdigest()}'
        )

        # Create the signing key
        signing_key = get_signature_key(self.secret_key, date_stamp, self.region, service)

        # Sign the string_to_sign using the signing key
        signature = hmac.new(signing_key, string_to_sign.encode("utf-8"), hashlib.sha256).hexdigest()

        # Add signing information to the request
        authorization_header = (
            f"{algorithm} Credential={self.access_key}/{credential_scope}, SignedHeaders={signed_headers}, "
            f"Signature={signature}"
        )

        # Headers
        headers = {
            "Content-Type": "application/json",
            "Authorization": authorization_header,
            "X-Amz-date": amz_date.encode(),
            "x-amz-content-sha256": payload_hash,
        }
        if self.session_token:
            headers["X-Amz-Security-Token"] = self.session_token

        return headers


class BedrockProvider(Provider):
    def __init__(self, client: AwsClient) -> None:
        self.client = client

    @classmethod
    def from_env(cls: Type["BedrockProvider"]) -> "BedrockProvider":
        aws_region = os.environ.get("AWS_REGION", "us-east-1")
        try:
            aws_access_key = os.environ["AWS_ACCESS_KEY_ID"]
            aws_secret_key = os.environ["AWS_SECRET_ACCESS_KEY"]
            aws_session_token = os.environ.get("AWS_SESSION_TOKEN")
        except KeyError:
            raise RuntimeError("Failed to get AWS_ACCESS_KEY_ID or AWS_SECRET_ACCESS_KEY from the environment")

        client = AwsClient(
            aws_region=aws_region,
            aws_access_key=aws_access_key,
            aws_secret_key=aws_secret_key,
            aws_session_token=aws_session_token,
        )
        return cls(client=client)

    def complete(
        self,
        model: str,
        system: str,
        messages: List[Message],
        tools: Tuple[Tool],
        **kwargs: Dict[str, Any],
    ) -> Tuple[Message, Usage]:
        """
        Generate a completion response from the Bedrock gateway.

        Args:
            model (str): The model identifier.
            system (str): The system prompt or configuration.
            messages (List[Message]): A list of messages to be processed by the model.
            tools (Tuple[Tool]): A tuple of tools to be used in the completion process.
            **kwargs: Additional keyword arguments for inference configuration.

        Returns:
            Tuple[Message, Usage]: A tuple containing the response message and usage data.
        """

        inference_config = dict(
            temperature=kwargs.pop("temperature", None),
            maxTokens=kwargs.pop("max_tokens", None),
            stopSequences=kwargs.pop("stop", None),
            topP=kwargs.pop("topP", None),
        )
        inference_config = {k: v for k, v in inference_config.items() if v is not None} or None

        converted_messages = [self.message_to_bedrock_spec(message) for message in messages]
        converted_system = [dict(text=system)]
        tool_config = self.tools_to_bedrock_spec(tools)
        payload = dict(
            system=converted_system,
            inferenceConfig=inference_config,
            messages=converted_messages,
            toolConfig=tool_config,
            **kwargs,
        )
        payload = {k: v for k, v in payload.items() if v}

        path = f"{self.client.host}model/{model}/converse"
        response = self._post(payload, path)
        response_message = response["output"]["message"]

        usage_data = response["usage"]
        usage = Usage(
            input_tokens=usage_data.get("inputTokens"),
            output_tokens=usage_data.get("outputTokens"),
            total_tokens=usage_data.get("totalTokens"),
        )

        return self.response_to_message(response_message), usage

    @retry_procedure
    def _post(self, payload: Any, path: str) -> dict:  # noqa: ANN401
        response = self.client.post(path, json=payload)
        return raise_for_status(response).json()

    @staticmethod
    def message_to_bedrock_spec(message: Message) -> dict:
        bedrock_content = []
        try:
            for content in message.content:
                if isinstance(content, Text):
                    bedrock_content.append({"text": content.text})
                elif isinstance(content, ToolUse):
                    for tool_use in message.tool_use:
                        bedrock_content.append(
                            {
                                "toolUse": {
                                    "toolUseId": tool_use.id,
                                    "name": tool_use.name,
                                    "input": tool_use.parameters,
                                }
                            }
                        )
                elif isinstance(content, ToolResult):
                    for tool_result in message.tool_result:
                        # try to parse the output as json
                        try:
                            output = json.loads(tool_result.output)
                            if isinstance(output, dict):
                                content = [{"json": output}]
                            else:
                                content = [{"text": str(output)}]
                        except json.JSONDecodeError:
                            content = [{"text": tool_result.output}]

                        bedrock_content.append(
                            {
                                "toolResult": {
                                    "toolUseId": tool_result.tool_use_id,
                                    "content": content,
                                    **({"status": "error"} if tool_result.is_error else {}),
                                }
                            }
                        )
            return {"role": message.role, "content": bedrock_content}

        except AttributeError:
            raise Exception("Invalid message")

    @staticmethod
    def response_to_message(response_message: dict) -> Message:
        content = []
        if response_message["role"] == "user":
            for block in response_message["content"]:
                if "text" in block:
                    content.append(Text(block["text"]))
                if "toolResult" in block:
                    content.append(
                        ToolResult(
                            tool_use_id=block["toolResult"]["toolResultId"],
                            output=block["toolResult"]["content"][0]["json"],
                            is_error=block["toolResult"].get("status") == "error",
                        )
                    )
            return Message(role="user", content=content)
        elif response_message["role"] == "assistant":
            for block in response_message["content"]:
                if "text" in block:
                    content.append(Text(block["text"]))
                if "toolUse" in block:
                    content.append(
                        ToolUse(
                            id=block["toolUse"]["toolUseId"],
                            name=block["toolUse"]["name"],
                            parameters=block["toolUse"]["input"],
                        )
                    )
            return Message(role="assistant", content=content)
        raise Exception("Invalid response")

    @staticmethod
    def tools_to_bedrock_spec(tools: Tuple[Tool]) -> Optional[dict]:
        if len(tools) == 0:
            return None  # API requires a non-empty tool config or None
        tools_added = set()
        tool_config_list = []
        for tool in tools:
            if tool.name in tools_added:
                logging.warning(f"Tool {tool.name} already added to tool config. Skipping.")
                continue
            tool_config_list.append(
                {
                    "toolSpec": {
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": {"json": tool.parameters},
                    }
                }
            )
            tools_added.add(tool.name)
        tool_config = {"tools": tool_config_list}
        return tool_config
