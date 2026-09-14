#!/usr/bin/env python3
"""
Provider Error Proxy - Simulates provider errors for testing Goose error handling.

This proxy intercepts HTTP traffic to AI providers and can inject errors interactively.
It supports the major providers: OpenAI, Anthropic, Google, OpenRouter, Tetrate, and Databricks.

Usage:
    uv run python proxy.py [--port PORT]

Interactive commands:
    n - No error (pass through) - permanent mode
    c - Context length exceeded error (1 error by default)
    c 4 - Context length exceeded error (4 errors in a row)
    c 0.3 or c 30% - Context length exceeded error (30% of requests)
    c * - Context length exceeded error (100% of requests)
    r - Rate limit error
    u - Unknown server error (500)
    q - Quit

To use with Goose, set the provider host environment variables:
    export OPENAI_HOST=http://localhost:8888
    export ANTHROPIC_HOST=http://localhost:8888
    # etc.
"""

import asyncio
import logging
import os
import random
import threading
from argparse import ArgumentParser
from enum import Enum
from typing import Optional

from aiohttp import web, ClientSession, ClientTimeout
from aiohttp.web import Request, Response, StreamResponse

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Provider endpoint mappings
PROVIDER_HOSTS = {
    'openai': 'https://api.openai.com',
    'anthropic': 'https://api.anthropic.com',
    'google': 'https://generativelanguage.googleapis.com',
    'openrouter': 'https://openrouter.ai',
    'tetrate': 'https://api.tetrate.io',
    'databricks': 'https://api.databricks.com',
}

# Paths that should always be forwarded without error injection
# These are typically authentication, configuration, or metadata endpoints
ALWAYS_FORWARD_PATHS = [
    '/oidc/',  # OIDC authentication endpoints
    '/.well-known/',  # Well-known endpoints for discovery
    '/oauth',  # OAuth endpoints
    '/api/2.0/',  # Databricks management API
]

# Path fragments identifying an inference/completion request.
#
# Error injection only applies to these. Every error mode the proxy offers
# describes a completion failure (context length exceeded, rate limited,
# upstream 500), so injecting them into metadata traffic such as model listing
# is meaningless. It is also actively harmful in count mode: a pre-flight
# request would silently consume the error budget intended for a completion,
# making "inject N errors" depend on goose's incidental call pattern rather
# than on the completions the test is actually exercising.
COMPLETION_PATHS = [
    '/chat/completions',  # OpenAI and compatible
    '/completions',       # OpenAI legacy
    '/messages',          # Anthropic
    '/responses',         # OpenAI Responses API
    ':generatecontent',   # Google Gemini
    ':streamgeneratecontent',
    '/converse',          # Bedrock
    '/invoke',            # Bedrock / Databricks serving
    '/invocations',
    '/serving-endpoints',  # Databricks
]

# Path suffixes identifying known non-inference (metadata) traffic. Used as
# the exclusion list when classifying requests that do not match
# COMPLETION_PATHS: a POST to an unrecognized path (e.g. a provider configured
# with a custom inference base path) is still treated as a completion, but
# known metadata endpoints are not. Matched as suffixes, not substrings, so a
# custom inference path that merely contains one of these segments (e.g.
# /v1/models/my-model/predict) is still classified as a completion.
METADATA_PATH_SUFFIXES = [
    '/models',      # model listing (OpenAI-compatible)
    '/api/tags',    # Ollama model listing
    '/api/show',    # Ollama model info (POSTed before completions for context limits)
    '/embeddings',  # embeddings are not turn completions
]


class ErrorMode(Enum):
    """Error injection modes."""
    NO_ERROR = 1
    CONTEXT_LENGTH = 2
    RATE_LIMIT = 3
    SERVER_ERROR = 4


# Error responses for each provider and error type
ERROR_CONFIGS = {
    'openai': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'error': {
                    'message': "This model's maximum context length is 128000 tokens. However, your messages resulted in 150000 tokens. Please reduce the length of the messages.",
                    'type': 'invalid_request_error',
                    'code': 'context_length_exceeded'
                }
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'error': {
                    'message': 'Rate limit exceeded. Please try again later.',
                    'type': 'rate_limit_error',
                    'code': 'rate_limit_exceeded'
                }
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 500,
            'body': {
                'error': {
                    'message': 'The server had an error while processing your request. Sorry about that!',
                    'type': 'server_error',
                    'code': 'internal_server_error'
                }
            }
        }
    },
    'anthropic': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'type': 'error',
                'error': {
                    'type': 'invalid_request_error',
                    'message': 'prompt is too long: 150000 tokens > 100000 maximum'
                }
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'type': 'error',
                'error': {
                    'type': 'rate_limit_error',
                    'message': 'Rate limit exceeded. Please try again later.'
                }
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 529,
            'body': {
                'type': 'error',
                'error': {
                    'type': 'overloaded_error',
                    'message': 'The API is temporarily overloaded. Please try again shortly.'
                }
            }
        }
    },
    'google': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'error': {
                    'code': 400,
                    'message': 'Request payload size exceeds the limit: 20000000 bytes.',
                    'status': 'INVALID_ARGUMENT'
                }
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'error': {
                    'code': 429,
                    'message': 'Resource has been exhausted (e.g. check quota).',
                    'status': 'RESOURCE_EXHAUSTED'
                }
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 503,
            'body': {
                'error': {
                    'code': 503,
                    'message': 'Service temporarily unavailable',
                    'status': 'UNAVAILABLE'
                }
            }
        }
    },
    'openrouter': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'error': {
                    'message': 'This model maximum context length is 128000 tokens, however you requested 150000 tokens',
                    'code': 400
                }
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'error': {
                    'message': 'Rate limit exceeded',
                    'code': 429
                }
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 500,
            'body': {
                'error': {
                    'message': 'Internal server error',
                    'code': 500
                }
            }
        }
    },
    'tetrate': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'error': {
                    'message': 'Request exceeds maximum context length',
                    'code': 'context_length_exceeded'
                }
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'error': {
                    'message': 'Rate limit exceeded',
                    'code': 'rate_limit_exceeded'
                }
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 503,
            'body': {
                'error': {
                    'message': 'Service unavailable',
                    'code': 'service_unavailable'
                }
            }
        }
    },
    'databricks': {
        ErrorMode.CONTEXT_LENGTH: {
            'status': 400,
            'body': {
                'error_code': 'INVALID_PARAMETER_VALUE',
                'message': 'The total number of tokens in the request exceeds the maximum allowed'
            }
        },
        ErrorMode.RATE_LIMIT: {
            'status': 429,
            'body': {
                'error_code': 'RATE_LIMIT_EXCEEDED',
                'message': 'Rate limit exceeded'
            }
        },
        ErrorMode.SERVER_ERROR: {
            'status': 500,
            'body': {
                'error_code': 'INTERNAL_ERROR',
                'message': 'Internal server error'
            }
        }
    }
}


class ErrorProxy:
    """HTTP proxy that can inject errors into provider responses."""
    
    def __init__(self):
        """Initialize the error proxy."""
        self.error_mode = ErrorMode.NO_ERROR
        self.error_count = 0  # Remaining errors to inject (0 = unlimited/percentage mode)
        self.error_percentage = 0.0  # Percentage of requests to error (0.0 = count mode)
        self.request_count = 0
        self.session: Optional[ClientSession] = None
        self.lock = threading.Lock()
        
    def set_error_mode(self, mode: ErrorMode, count: int = 1, percentage: float = 0.0):
        """
        Set the error injection mode.
        
        Args:
            mode: The error mode to use
            count: Number of errors to inject (default 1, 0 for unlimited)
            percentage: Percentage of requests to error (0.0-1.0, 0.0 for count mode)
        """
        with self.lock:
            self.error_mode = mode
            self.error_count = count
            self.error_percentage = percentage
            
    def should_inject_error(self) -> bool:
        """
        Determine if we should inject an error for this request.
        
        Returns:
            True if an error should be injected, False otherwise
        """
        with self.lock:
            if self.error_mode == ErrorMode.NO_ERROR:
                return False
                
            # Percentage mode
            if self.error_percentage > 0.0:
                return random.random() < self.error_percentage
            
            # Count mode
            if self.error_count > 0:
                self.error_count -= 1
                # If this was the last error, switch back to NO_ERROR
                if self.error_count == 0:
                    self.error_mode = ErrorMode.NO_ERROR
                return True
            elif self.error_count == 0 and self.error_percentage == 0.0:
                # Count reached zero, switch back to NO_ERROR
                self.error_mode = ErrorMode.NO_ERROR
                return False

            return False
            
    def get_error_mode(self) -> ErrorMode:
        """Get the current error injection mode."""
        with self.lock:
            return self.error_mode
    
    def get_error_config(self) -> tuple[ErrorMode, int, float]:
        """Get the current error configuration."""
        with self.lock:
            return (self.error_mode, self.error_count, self.error_percentage)
        
    async def start_session(self):
        """Start the aiohttp client session."""
        timeout = ClientTimeout(total=600)  # Match provider timeout
        self.session = ClientSession(timeout=timeout)
        
    async def close_session(self):
        """Close the aiohttp client session."""
        if self.session:
            await self.session.close()
            
    def detect_provider(self, request: Request) -> str:
        """
        Detect which provider this request is for based on headers and path.

        Args:
            request: The incoming HTTP request

        Returns:
            Provider name
        """
        path = request.path.lower()

        # Check for databricks-specific paths first (before header checks)
        if '/serving-endpoints/' in path or '/api/2.0/' in path or '/oidc/' in path:
            return 'databricks'

        # Check for provider-specific headers
        if 'x-api-key' in request.headers:
            return 'anthropic'
        if 'x-goog-api-key' in request.headers:
            return 'google'
        if 'authorization' in request.headers:
            auth = request.headers['authorization'].lower()
            if 'bearer' in auth:
                # Most providers use bearer tokens, check path for hints
                if 'anthropic' in path or 'messages' in path:
                    return 'anthropic'
                if 'google' in path or 'generativelanguage' in path:
                    return 'google'
                if 'openrouter' in path:
                    return 'openrouter'
                if 'tetrate' in path:
                    return 'tetrate'
                if 'databricks' in path:
                    return 'databricks'
                # Default to openai for bearer tokens
                return 'openai'

        # Default to openai if we can't determine
        return 'openai'
        
    def should_always_forward(self, request: Request) -> bool:
        """
        Check if this request should always be forwarded (never injected with errors).

        Args:
            request: The incoming HTTP request

        Returns:
            True if request should always be forwarded
        """
        path = request.path
        for forward_path in ALWAYS_FORWARD_PATHS:
            if forward_path in path:
                return True
        return not self.is_completion_request(request)

    def is_completion_request(self, request: Request) -> bool:
        """
        Check whether this request is an inference/completion call.

        Only completion requests are eligible for error injection; see
        COMPLETION_PATHS.

        Args:
            request: The incoming HTTP request

        Returns:
            True if this is a completion request
        """
        path = request.path.lower()
        if any(fragment in path for fragment in COMPLETION_PATHS):
            return True
        # Providers configured with a custom inference base path (e.g. an
        # OpenAI-compatible endpoint at "<project_id>/v1") post completions to
        # routes that match none of the known fragments. Treat any other POST
        # that is not known metadata traffic as a completion so error
        # injection still applies to custom inference routes.
        if request.method.upper() != 'POST':
            return False
        path = path.rstrip('/')
        return not any(path.endswith(suffix) for suffix in METADATA_PATH_SUFFIXES)

    def get_target_url(self, request: Request, provider: str) -> str:
        """
        Construct the target URL for the provider.

        Args:
            request: The incoming HTTP request
            provider: The detected provider name

        Returns:
            Full target URL
        """
        # Check for provider-specific real host in environment
        real_host_env = f"{provider.upper()}_REAL_HOST"
        base_host = os.environ.get(real_host_env)

        # If no provider-specific real host and this is an always-forward path,
        # check if ANY *_REAL_HOST is set (for auth endpoints where provider detection might fail)
        if base_host is None and self.should_always_forward(request):
            for provider_name in PROVIDER_HOSTS.keys():
                env_var = f"{provider_name.upper()}_REAL_HOST"
                if env_var in os.environ:
                    base_host = os.environ[env_var]
                    logger.info(f"Using {env_var} for always-forward path")
                    break

        # Fall back to default provider host
        if base_host is None:
            base_host = PROVIDER_HOSTS.get(provider, PROVIDER_HOSTS['openai'])

        path = request.path
        query = request.query_string

        url = f"{base_host}{path}"
        if query:
            url = f"{url}?{query}"

        return url
        
    def _format_status_line(self) -> str:
        """Format a one-line status indicator."""
        mode, count, percentage = self.get_error_config()
        mode_symbols = {
            ErrorMode.NO_ERROR: "✅",
            ErrorMode.CONTEXT_LENGTH: "📏",
            ErrorMode.RATE_LIMIT: "⏱️",
            ErrorMode.SERVER_ERROR: "💥"
        }

        symbol = mode_symbols.get(mode, "❓")
        mode_name = mode.name.replace('_', ' ').title()

        if mode == ErrorMode.NO_ERROR:
            return f"{symbol} {mode_name}"
        elif percentage > 0.0:
            return f"{symbol} {mode_name} ({percentage*100:.0f}%)"
        elif count > 0:
            return f"{symbol} {mode_name} ({count} remaining)"
        else:
            return f"{symbol} {mode_name}"

    async def handle_request(self, request: Request) -> Response:
        """
        Handle an incoming HTTP request.

        Args:
            request: The incoming HTTP request

        Returns:
            HTTP response (either proxied or error)
        """
        self.request_count += 1
        provider = self.detect_provider(request)

        logger.info(f"📨 Request #{self.request_count}: {request.method} {request.path} -> {provider}")

        # Check if this request should always be forwarded
        if self.should_always_forward(request):
            if self.is_completion_request(request):
                logger.info(f"🔄 Always forwarding: {request.path}")
            else:
                logger.info(
                    f"🔄 Forwarding non-completion request (not eligible for "
                    f"error injection): {request.path}"
                )
        else:
            # Capture the error mode BEFORE checking if we should inject (since that modifies state)
            mode_before_check = self.get_error_mode()

            # Check if we should inject an error
            should_error = self.should_inject_error()
            if should_error:
                # Use the mode captured before the check, since should_inject_error may have changed it
                error_config = ERROR_CONFIGS.get(provider, ERROR_CONFIGS['openai']).get(
                    mode_before_check, ERROR_CONFIGS['openai'][ErrorMode.SERVER_ERROR]
                )
                logger.warning(f"💥 Injecting {mode_before_check.name} error (status {error_config['status']}) for {provider}")
                # Show status after the injection to reflect the updated state
                logger.info(f"Status: {self._format_status_line()}")
                return web.json_response(
                    error_config['body'],
                    status=error_config['status']
                )
        
        # Forward the request to the actual provider
        target_url = self.get_target_url(request, provider)
        
        try:
            # Read request body
            body = await request.read()
            
            # Copy headers, excluding hop-by-hop headers
            headers = {k: v for k, v in request.headers.items() 
                      if k.lower() not in ('host', 'connection', 'keep-alive', 
                                           'proxy-authenticate', 'proxy-authorization',
                                           'te', 'trailers', 'transfer-encoding', 'upgrade')}
            
            # Make the proxied request
            async with self.session.request(
                method=request.method,
                url=target_url,
                headers=headers,
                data=body,
                allow_redirects=False
            ) as resp:
                # Copy response headers
                # For non-streaming responses, we need to exclude content-encoding and content-length
                # because aiohttp.Response.read() automatically decompresses the body
                response_headers = {k: v for k, v in resp.headers.items()
                                   if k.lower() not in ('connection', 'keep-alive',
                                                        'transfer-encoding', 'content-encoding',
                                                        'content-length')}
                
                # Check if this is a streaming response (SSE)
                content_type = resp.headers.get('content-type', '').lower()
                is_streaming = 'text/event-stream' in content_type
                
                if is_streaming:
                    # Stream the response (Server-Sent Events)
                    logger.info(f"🌊 Streaming response: {resp.status}")
                    response = StreamResponse(
                        status=resp.status,
                        headers=response_headers
                    )
                    await response.prepare(request)

                    # Stream chunks from provider to client
                    try:
                        async for chunk in resp.content.iter_any():
                            await response.write(chunk)
                        await response.write_eof()
                    except Exception as stream_error:
                        logger.warning(f"Stream write error (client may have disconnected): {stream_error}")
                    logger.info(f"Status: {self._format_status_line()}")
                    return response
                else:
                    # Non-streaming response - read entire body
                    response_body = await resp.read()
                    logger.info(f"✅ Proxied response: {resp.status}")
                    logger.info(f"Status: {self._format_status_line()}")

                    return Response(
                        body=response_body,
                        status=resp.status,
                        headers=response_headers
                    )
                
        except Exception as e:
            logger.error(f"❌ Error proxying request: {e}", exc_info=True)
            return web.json_response(
                {'error': {'message': f'Proxy error: {str(e)}'}},
                status=500
            )


def parse_command(command: str) -> tuple[Optional[ErrorMode], int, float, Optional[str]]:
    """
    Parse a command string and return the error mode, count, and percentage.

    Args:
        command: Command string (e.g., "c", "c 3", "r 30%", "u *")

    Returns:
        Tuple of (mode, count, percentage, error_message)
        If error_message is not None, parsing failed
    """
    # Parse command - remove all whitespace and parse
    command_no_space = command.strip().replace(" ", "")
    if not command_no_space:
        return (None, 0, 0.0, "Empty command")

    # Get the first character (error type letter)
    error_letter = command_no_space[0].lower()

    # Map letter to ErrorMode
    mode_map = {
        'n': ErrorMode.NO_ERROR,
        'c': ErrorMode.CONTEXT_LENGTH,
        'r': ErrorMode.RATE_LIMIT,
        'u': ErrorMode.SERVER_ERROR
    }

    if error_letter not in mode_map:
        return (None, 0, 0.0, f"Invalid command: '{error_letter}'. Use n, c, r, or u")

    mode = mode_map[error_letter]

    # Parse the rest as count or percentage
    count = 1
    percentage = 0.0

    if len(command_no_space) > 1:
        value_str = command_no_space[1:]

        try:
            # Check for * (100%)
            if value_str == '*':
                percentage = 1.0
                count = 0  # Percentage mode
            # Check for percentage with % sign (e.g., "30%")
            elif value_str.endswith('%'):
                percentage = float(value_str[:-1]) / 100.0
                if percentage < 0.0 or percentage > 1.0:
                    return (None, 0, 0.0, f"Invalid percentage: {percentage*100:.0f}%. Must be between 0% and 100%")
                count = 0  # Percentage mode
            # Check if it's a decimal (percentage as 0.0-1.0)
            elif '.' in value_str:
                percentage = float(value_str)
                if percentage < 0.0 or percentage > 1.0:
                    return (None, 0, 0.0, f"Invalid percentage: {percentage}. Must be between 0.0 and 1.0")
                count = 0  # Percentage mode
            else:
                # It's an integer count
                count = int(value_str)
                if count < 0:
                    return (None, 0, 0.0, f"Invalid count: {count}. Must be >= 0")
        except ValueError:
            return (None, 0, 0.0, f"Invalid value: '{value_str}'. Must be an integer, decimal, percentage (30%), or * (100%)")

    return (mode, count, percentage, None)


def print_status(proxy: ErrorProxy):
    """Print the current proxy status."""
    mode, count, percentage = proxy.get_error_config()
    mode_names = {
        ErrorMode.NO_ERROR: "✅ No error (pass through)",
        ErrorMode.CONTEXT_LENGTH: "📏 Context length exceeded",
        ErrorMode.RATE_LIMIT: "⏱️  Rate limit exceeded",
        ErrorMode.SERVER_ERROR: "💥 Server error (500)"
    }

    print("\n" + "=" * 60)
    mode_str = mode_names.get(mode, 'Unknown')
    if mode != ErrorMode.NO_ERROR:
        if percentage > 0.0:
            mode_str += f" ({percentage*100:.0f}% of requests)"
        elif count > 0:
            mode_str += f" ({count} remaining)"
    print(f"Current mode: {mode_str}")
    print(f"Requests handled: {proxy.request_count}")
    print("=" * 60)
    print("\nCommands:")
    print("  n      - No error (pass through) - permanent")
    print("  c      - Context length exceeded (1 time)")
    print("  c 4    - Context length exceeded (4 times)")
    print("  c 0.3  - Context length exceeded (30% of requests)")
    print("  c 30%  - Context length exceeded (30% of requests)")
    print("  c *    - Context length exceeded (100% of requests)")
    print("  r      - Rate limit error (1 time)")
    print("  u      - Unknown server error (1 time)")
    print("  q      - Quit")
    print()


def stdin_reader(proxy: ErrorProxy, loop):
    """Read commands from stdin in a separate thread."""
    print_status(proxy)

    while True:
        try:
            command = input("Enter command: ").strip()

            if command.lower() == 'q':
                print("\n🛑 Shutting down proxy...")
                # Schedule the shutdown in the event loop
                asyncio.run_coroutine_threadsafe(shutdown_server(loop), loop)
                break

            # Parse the command using the shared parser
            mode, count, percentage, error_msg = parse_command(command)

            if error_msg:
                print(f"❌ {error_msg}")
                continue

            # Set the error mode
            proxy.set_error_mode(mode, count, percentage)
            print_status(proxy)

        except EOFError:
            # Handle Ctrl+D
            print("\n🛑 Shutting down proxy...")
            asyncio.run_coroutine_threadsafe(shutdown_server(loop), loop)
            break
        except Exception as e:
            logger.error(f"Error reading stdin: {e}")


async def shutdown_server(loop):
    """Shutdown the server gracefully."""
    # Stop the event loop
    loop.stop()


async def create_app(proxy: ErrorProxy) -> web.Application:
    """
    Create the aiohttp application.
    
    Args:
        proxy: The ErrorProxy instance
        
    Returns:
        Configured aiohttp application
    """
    app = web.Application()
    
    # Setup and teardown
    async def on_startup(app):
        await proxy.start_session()
        logger.info("🚀 Proxy session started")
        
    async def on_cleanup(app):
        await proxy.close_session()
        logger.info("🛑 Proxy session closed")
        
    app.on_startup.append(on_startup)
    app.on_cleanup.append(on_cleanup)
    
    # Route all requests through the proxy
    app.router.add_route('*', '/{path:.*}', proxy.handle_request)
    
    return app


def main():
    """Main entry point."""
    parser = ArgumentParser(description='Provider Error Proxy for Goose testing')
    parser.add_argument(
        '--port',
        type=int,
        default=8888,
        help='Port to listen on (default: 8888)'
    )
    parser.add_argument(
        '--mode',
        type=str,
        help='Error mode command (e.g., "c 3", "r 30%%", "u *", "n")'
    )
    parser.add_argument(
        '--no-stdin',
        action='store_true',
        help='Disable stdin reader (for background/automated mode)'
    )

    args = parser.parse_args()
    
    print("=" * 60)
    print("🔧 Provider Error Proxy")
    print("=" * 60)
    print(f"Port: {args.port}")
    print()
    print("To use with Goose, set these environment variables:")
    print(f"  export OPENAI_HOST=http://localhost:{args.port}")
    print(f"  export ANTHROPIC_HOST=http://localhost:{args.port}")
    print(f"  export GOOGLE_HOST=http://localhost:{args.port}")
    print(f"  export OPENROUTER_HOST=http://localhost:{args.port}")
    print(f"  export TETRATE_HOST=http://localhost:{args.port}")
    print(f"  export DATABRICKS_HOST=http://localhost:{args.port}")
    print("=" * 60)
    
    # Create proxy instance
    proxy = ErrorProxy()

    # Set initial error mode from command-line arguments
    if args.mode:
        mode, count, percentage, error_msg = parse_command(args.mode)

        if error_msg:
            print(f"❌ Error parsing --mode argument: {error_msg}")
            print(f"   Example usage: --mode \"c 3\" or --mode \"r 30%\"")
            return

        proxy.set_error_mode(mode, count, percentage)
        print()
        print(f"Initial mode set from command-line arguments:")
        print(f"  Mode: {mode.name}")
        if percentage > 0.0:
            print(f"  Percentage: {percentage*100:.0f}%")
        elif count > 0:
            print(f"  Count: {count}")
        print()

    # Create event loop
    loop = asyncio.new_event_loop()
    asyncio.set_event_loop(loop)

    # Start stdin reader thread only if not disabled
    if not args.no_stdin:
        stdin_thread = threading.Thread(target=stdin_reader, args=(proxy, loop), daemon=True)
        stdin_thread.start()
    else:
        print("Running in no-stdin mode (background/automated)")
        print("Use SIGINT (Ctrl+C) or SIGTERM to stop the proxy")
        print()
    
    # Create and run the app
    app = loop.run_until_complete(create_app(proxy))
    
    # Run the web server
    runner = web.AppRunner(app)
    loop.run_until_complete(runner.setup())
    site = web.TCPSite(runner, 'localhost', args.port)
    loop.run_until_complete(site.start())
    
    logger.info(f"Proxy running on http://localhost:{args.port}")
    
    try:
        loop.run_forever()
    except KeyboardInterrupt:
        print("\n🛑 Shutting down proxy...")
    finally:
        loop.run_until_complete(runner.cleanup())
        loop.close()


if __name__ == '__main__':
    main()
